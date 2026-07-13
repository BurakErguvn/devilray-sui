use crate::collectors::collector_for_dex_name;
use crate::discovery::{
    ALL_DISCOVERY_SPECS, DiscoveryEventRef, DiscoveryObjectRef, DiscoveryPageCommit,
    OBJECT_BOOTSTRAP_SOURCE_KEY, fetch_graphql_available_range, scan_dex_events,
    scan_dex_object_bootstrap,
};
use crate::models::{PoolState, Token};
use crate::queue::{MessageQueueTrait, QueueMessage};
use crate::storage::{PostgresStorageTrait, RedisCacheTrait};
use crate::sui_client::SuiClientTrait;
use crate::workers::StaticWorkerConfig;
use anyhow::{Result, anyhow};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use tokio::sync::Semaphore;

pub struct StaticPoolManager {
    postgres_db: Arc<dyn PostgresStorageTrait>,
    redis_cache: Arc<dyn RedisCacheTrait>,
    sui_client: Arc<dyn SuiClientTrait>,
    queue: Arc<dyn MessageQueueTrait>,
    queue_name: String,
    config: StaticWorkerConfig,
    scan_cycle: AtomicU32,
    topology_ready: Arc<AtomicBool>,
    hydrate_semaphore: Arc<Semaphore>,
}

impl StaticPoolManager {
    pub fn new(
        postgres_db: Arc<dyn PostgresStorageTrait>,
        redis_cache: Arc<dyn RedisCacheTrait>,
        sui_client: Arc<dyn SuiClientTrait>,
        queue: Arc<dyn MessageQueueTrait>,
        queue_name: String,
        config: StaticWorkerConfig,
        topology_ready: Arc<AtomicBool>,
    ) -> Self {
        let concurrency = config.discovery_hydrate_concurrency.max(1);
        Self {
            postgres_db,
            redis_cache,
            sui_client,
            queue,
            queue_name,
            config,
            scan_cycle: AtomicU32::new(0),
            topology_ready,
            hydrate_semaphore: Arc::new(Semaphore::new(concurrency)),
        }
    }

    pub fn topology_ready(&self) -> Arc<AtomicBool> {
        self.topology_ready.clone()
    }

    /// Runs the static worker loop. Periodically scans for new pools and registers them.
    pub async fn run(&self, shutdown_rx: &mut tokio::sync::watch::Receiver<bool>) -> Result<()> {
        tracing::info!("Static Pool Manager started.");

        if let Err(err) = self.discover_and_register().await {
            tracing::error!("Initial pool discovery failed: {:?}", err);
        }

        loop {
            if *shutdown_rx.borrow() {
                tracing::info!("Static Pool Manager shutdown signal received.");
                break;
            }

            tracing::info!("Starting pool and token discovery scan...");
            if let Err(err) = self.discover_and_register().await {
                tracing::error!("Pool discovery failed: {:?}", err);
            }

            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(self.config.scan_interval_secs)) => {}
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        break;
                    }
                }
            }
        }

        tracing::info!("Static Pool Manager clean exit.");
        Ok(())
    }

    /// Object-bootstrap primary path + event incremental acceleration.
    pub async fn discover_and_register(&self) -> Result<()> {
        let existing_pools = self.postgres_db.list_pools().await?;
        let event_range = match fetch_graphql_available_range(self.sui_client.as_ref()).await {
            Ok(range) => {
                tracing::info!(
                    first = ?range.first_checkpoint,
                    last = ?range.last_checkpoint,
                    "GraphQL available checkpoint range"
                );
                range
            }
            Err(err) => {
                tracing::warn!("Could not fetch GraphQL serviceConfig range: {:?}", err);
                if existing_pools.is_empty() {
                    tracing::error!(
                        "No pools in database and GraphQL unavailable; discovery cannot proceed"
                    );
                    return Err(err);
                }
                tracing::warn!(
                    pool_count = existing_pools.len(),
                    "Continuing in degraded mode with existing PostgreSQL topology"
                );
                crate::discovery::scanner::GraphqlAvailableRange {
                    first_checkpoint: None,
                    last_checkpoint: None,
                }
            }
        };

        let mut bootstrap_ok = false;
        let manager = self;

        for spec in ALL_DISCOVERY_SPECS {
            let postgres = manager.postgres_db.clone();
            let sui = manager.sui_client.clone();
            let config = manager.config.clone();
            let mgr = manager;

            let after_commit = {
                let redis = mgr.redis_cache.clone();
                let queue = mgr.queue.clone();
                let queue_name = mgr.queue_name.clone();
                move |commit: &DiscoveryPageCommit| {
                    let pools = commit.pools.clone();
                    let redis = redis.clone();
                    let queue = queue.clone();
                    let queue_name = queue_name.clone();
                    tokio::spawn(async move {
                        for pool in pools {
                            if let Err(err) = redis.set_pool_state(&pool).await {
                                tracing::error!("hot-path redis update failed: {:?}", err);
                                continue;
                            }
                            if let Err(err) = queue
                                .publish(&queue_name, &QueueMessage::PoolStateUpdate(pool))
                                .await
                            {
                                tracing::error!("hot-path queue publish failed: {:?}", err);
                            }
                        }
                    });
                }
            };

            match scan_dex_object_bootstrap(
                spec,
                sui.as_ref(),
                postgres.as_ref(),
                &config,
                |object_ref| async move { mgr.hydrate_object_ref(object_ref).await },
                Some(after_commit),
            )
            .await
            {
                Ok(stats) => {
                    if stats.objects_seen > 0 || stats.pages_scanned > 0 {
                        bootstrap_ok = true;
                    }
                    tracing::info!(
                        dex = spec.dex_name,
                        source = "object_bootstrap",
                        pages = stats.pages_scanned,
                        objects = stats.objects_seen,
                        pools = stats.pools_registered,
                        failures = stats.failures_recorded,
                        "DEX object bootstrap finished"
                    );
                }
                Err(err) => {
                    tracing::error!(dex = spec.dex_name, error = ?err, "DEX object bootstrap failed");
                    if existing_pools.is_empty() {
                        return Err(err);
                    }
                }
            }
        }

        for spec in ALL_DISCOVERY_SPECS {
            let postgres = manager.postgres_db.clone();
            let sui = manager.sui_client.clone();
            let config = manager.config.clone();
            let mgr = manager;
            let range = event_range.clone();

            let after_commit = {
                let redis = mgr.redis_cache.clone();
                let queue = mgr.queue.clone();
                let queue_name = mgr.queue_name.clone();
                move |commit: &DiscoveryPageCommit| {
                    let pools = commit.pools.clone();
                    let redis = redis.clone();
                    let queue = queue.clone();
                    let queue_name = queue_name.clone();
                    tokio::spawn(async move {
                        for pool in pools {
                            if let Err(err) = redis.set_pool_state(&pool).await {
                                tracing::error!("hot-path redis update failed: {:?}", err);
                                continue;
                            }
                            if let Err(err) = queue
                                .publish(&queue_name, &QueueMessage::PoolStateUpdate(pool))
                                .await
                            {
                                tracing::error!("hot-path queue publish failed: {:?}", err);
                            }
                        }
                    });
                }
            };

            match scan_dex_events(
                spec,
                sui.as_ref(),
                postgres.as_ref(),
                &config,
                &range,
                |event_ref| async move { mgr.hydrate_event_ref(event_ref).await },
                Some(after_commit),
            )
            .await
            {
                Ok(stats) => {
                    tracing::info!(
                        dex = spec.dex_name,
                        source = "event_incremental",
                        pages = stats.pages_scanned,
                        events = stats.events_seen,
                        pools = stats.pools_registered,
                        failures = stats.failures_recorded,
                        "DEX event incremental scan finished"
                    );
                }
                Err(err) => {
                    tracing::warn!(dex = spec.dex_name, error = ?err, "DEX event incremental scan failed");
                }
            }
        }

        let pools_after = self.postgres_db.list_pools().await?;
        if bootstrap_ok || !pools_after.is_empty() {
            self.topology_ready.store(true, Ordering::SeqCst);
        } else if pools_after.is_empty() {
            self.topology_ready.store(false, Ordering::SeqCst);
            return Err(anyhow!(
                "discovery produced no results and database is empty"
            ));
        }

        self.retry_discovery_failures().await?;

        let cycle = self.scan_cycle.fetch_add(1, Ordering::SeqCst) + 1;
        if self.config.reconciliation_every_n_scans > 0
            && cycle.is_multiple_of(self.config.reconciliation_every_n_scans)
        {
            let pools = self.postgres_db.list_pools().await?;
            let mut updated = 0u32;
            for pool in pools {
                if self.hydrate_existing_pool(&pool).await.is_ok() {
                    updated += 1;
                }
            }
            tracing::info!(updated, "Reconciled existing pools from chain");
        }

        self.refresh_topology_caches().await?;

        match self.sui_client.get_reference_gas_price().await {
            Ok(price) => {
                tracing::info!("Fetched current reference gas price: {} MIST", price);
                if let Err(err) = self.postgres_db.set_reference_gas_price(price).await {
                    tracing::error!("Failed to store reference gas price in database: {:?}", err);
                }
                if let Err(err) = self.redis_cache.set_reference_gas_price(price).await {
                    tracing::error!("Failed to cache reference gas price: {:?}", err);
                }
            }
            Err(err) => {
                tracing::error!(
                    "Failed to fetch reference gas price from Sui RPC: {:?}",
                    err
                );
            }
        }

        self.refresh_tick_data().await?;
        Ok(())
    }

    async fn apply_hot_path_commit(&self, commit: &DiscoveryPageCommit) -> Result<()> {
        for pool in &commit.pools {
            self.redis_cache.set_pool_state(pool).await?;
            self.queue
                .publish(
                    &self.queue_name,
                    &QueueMessage::PoolStateUpdate(pool.clone()),
                )
                .await?;
        }
        Ok(())
    }

    async fn hydrate_object_ref(
        &self,
        object_ref: DiscoveryObjectRef,
    ) -> Result<DiscoveryPageCommit> {
        let _permit = self
            .hydrate_semaphore
            .acquire()
            .await
            .map_err(|e| anyhow!("hydrate semaphore closed: {e}"))?;
        self.build_commit_for_pool(&object_ref.pool_id, &object_ref.dex_name, None)
            .await
    }

    async fn hydrate_event_ref(&self, event_ref: DiscoveryEventRef) -> Result<DiscoveryPageCommit> {
        let pool_ref = event_ref
            .pool_ref
            .ok_or_else(|| anyhow!("missing pool ref for event {}", event_ref.event_id))?;
        let _permit = self
            .hydrate_semaphore
            .acquire()
            .await
            .map_err(|e| anyhow!("hydrate semaphore closed: {e}"))?;
        self.build_commit_for_pool(
            &pool_ref.pool_id,
            &pool_ref.dex_name,
            Some(event_ref.event_id),
        )
        .await
    }

    async fn build_commit_for_pool(
        &self,
        pool_id: &str,
        dex_name: &str,
        resolved_event_id: Option<String>,
    ) -> Result<DiscoveryPageCommit> {
        let Some(collector) = collector_for_dex_name(dex_name) else {
            return Err(anyhow!("no collector for DEX {}", dex_name));
        };

        let pool = collector
            .fetch_pool(self.sui_client.as_ref(), pool_id)
            .await?;

        let mut tokens = Vec::new();
        for coin in [&pool.coin_type_a, &pool.coin_type_b] {
            if let Some(token) = self.fetch_token_if_missing(coin).await? {
                tokens.push(token);
            }
        }

        let mut commit = DiscoveryPageCommit {
            pools: vec![pool],
            tokens,
            ..Default::default()
        };
        commit.progress.pools_discovered = 1;
        if let Some(event_id) = resolved_event_id {
            commit.resolved_failure_ids.push(event_id);
        }

        tracing::info!(
            pool_id = %pool_id,
            dex = %dex_name,
            "Hydrated verified pool from discovery"
        );
        Ok(commit)
    }

    async fn fetch_token_if_missing(&self, address: &str) -> Result<Option<Token>> {
        if self.postgres_db.get_token(address).await?.is_some() {
            return Ok(None);
        }
        let meta = self.sui_client.get_coin_metadata(address).await?;
        let symbol = meta
            .get("symbol")
            .and_then(|s| s.as_str())
            .unwrap_or("UNKNOWN")
            .to_string();
        let name = meta
            .get("name")
            .and_then(|s| s.as_str())
            .unwrap_or("Unknown")
            .to_string();
        let decimals = meta.get("decimals").and_then(|d| d.as_u64()).unwrap_or(9) as u8;
        Ok(Some(Token {
            address: address.to_string(),
            symbol,
            name,
            decimals,
        }))
    }

    async fn hydrate_existing_pool(&self, pool: &PoolState) -> Result<()> {
        let commit = self
            .build_commit_for_pool(&pool.pool_id, &pool.dex_name, None)
            .await?;
        self.postgres_db.commit_discovery_page(&commit).await?;
        self.apply_hot_path_commit(&commit).await?;
        Ok(())
    }

    async fn retry_discovery_failures(&self) -> Result<()> {
        for spec in ALL_DISCOVERY_SPECS {
            let failures = self
                .postgres_db
                .list_pool_discovery_failures(spec.dex_name, 100)
                .await?;
            for failure in failures {
                let Some(pool_id) = failure.pool_id.as_deref() else {
                    continue;
                };
                match self
                    .build_commit_for_pool(pool_id, spec.dex_name, Some(failure.event_id.clone()))
                    .await
                {
                    Ok(commit) => {
                        self.postgres_db.commit_discovery_page(&commit).await?;
                        self.apply_hot_path_commit(&commit).await?;
                        tracing::info!(
                            event_id = %failure.event_id,
                            pool_id = %pool_id,
                            "Retry succeeded for discovery failure"
                        );
                    }
                    Err(err) => {
                        tracing::debug!(
                            event_id = %failure.event_id,
                            pool_id = %pool_id,
                            error = %err,
                            "Retry still failing"
                        );
                    }
                }
            }
        }
        Ok(())
    }

    async fn refresh_topology_caches(&self) -> Result<()> {
        let pools = self.postgres_db.list_pools().await?;
        let active: Vec<PoolState> = pools.iter().filter(|p| !p.is_paused).cloned().collect();
        self.redis_cache.set_active_pools(&active).await?;

        let tokens = self.postgres_db.list_tokens().await?;
        self.redis_cache.set_all_tokens(&tokens).await?;
        tracing::info!(
            active_pools = active.len(),
            tokens = tokens.len(),
            "Refreshed Redis topology snapshots"
        );
        Ok(())
    }

    async fn refresh_tick_data(&self) -> Result<()> {
        let pools = self.postgres_db.list_pools().await?;
        let mut seen = HashSet::new();

        for pool in pools {
            if !seen.insert(pool.pool_id.clone()) {
                continue;
            }
            let Some(collector) = collector_for_dex_name(&pool.dex_name) else {
                continue;
            };

            match collector
                .fetch_tick_data(self.sui_client.as_ref(), &pool.pool_id)
                .await
            {
                Ok(tick_data) => {
                    if let Err(e) = self.redis_cache.set_pool_tick_data(&tick_data).await {
                        tracing::error!(
                            "Failed to cache tick data for pool {}: {:?}",
                            pool.pool_id,
                            e
                        );
                    }
                    if let Err(e) = self
                        .queue
                        .publish(
                            &self.queue_name,
                            &QueueMessage::PoolTickDataUpdate(tick_data),
                        )
                        .await
                    {
                        tracing::error!(
                            "Failed to queue tick data for pool {}: {:?}",
                            pool.pool_id,
                            e
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Tick fetch failed for {} pool {}: {:?}",
                        pool.dex_name,
                        pool.pool_id,
                        e
                    );
                }
            }
        }

        Ok(())
    }

    /// Returns whether all DEX object bootstraps completed at least once.
    pub async fn all_bootstraps_complete(&self) -> Result<bool> {
        for spec in ALL_DISCOVERY_SPECS {
            let progress = self
                .postgres_db
                .get_pool_discovery_progress(spec.dex_name, OBJECT_BOOTSTRAP_SOURCE_KEY)
                .await?;
            if progress.is_none_or(|p| !p.bootstrap_complete) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::fixtures;
    use crate::queue::tests::InMemoryMessageQueue;
    use crate::storage::postgres::tests::InMemoryPostgresStorage;
    use crate::storage::redis::tests::InMemoryRedisCache;
    use crate::sui_client::tests::MockSuiClient;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    fn mock_cetus_pool_object(pool_id: &str) -> serde_json::Value {
        json!({
            "data": {
                "objectId": pool_id,
                "content": {
                    "type": "0x1eabed72c53feb3805120a081dc15963c204dc8d091542592abaf7a35689b2fb::pool::Pool<0x2::sui::SUI, 0x5d4b3::coin::COIN>",
                    "fields": {
                        "current_sqrt_price": "18446744073709551616",
                        "liquidity": "50000000",
                        "is_pause": false,
                        "fee_rate": "3000"
                    }
                }
            }
        })
    }

    #[tokio::test]
    async fn test_static_worker_object_bootstrap_hydrates_from_chain() {
        let postgres = Arc::new(InMemoryPostgresStorage::new());
        let redis = Arc::new(InMemoryRedisCache::new());
        let queue = Arc::new(InMemoryMessageQueue::new());
        let sui = Arc::new(MockSuiClient::new());
        let topology_ready = Arc::new(AtomicBool::new(false));

        let pool_id = fixtures::CETUS_POOL_ID;
        *sui.query_graphql_with_variables_mock.lock().unwrap() = Box::new(move |query, _| {
            if query.contains("serviceConfig") {
                Ok(json!({
                    "serviceConfig": {
                        "eventsAvailableRange": {
                            "first": { "sequenceNumber": 1 },
                            "last": { "sequenceNumber": 99999999 }
                        }
                    }
                }))
            } else if query.contains("objects") {
                Ok(fixtures::cetus_object_graphql_response())
            } else {
                Ok(json!({
                    "events": {
                        "pageInfo": { "hasNextPage": false, "endCursor": null },
                        "nodes": []
                    }
                }))
            }
        });
        *sui.get_object_mock.lock().unwrap() = {
            let pool_id = pool_id.to_string();
            Box::new(move |id| {
                assert_eq!(id, pool_id);
                Ok(mock_cetus_pool_object(&pool_id))
            })
        };

        let config = StaticWorkerConfig {
            discovery_max_pages_per_scan: 1,
            discovery_inter_page_ms: 0,
            reconciliation_every_n_scans: 0,
            ..StaticWorkerConfig::default()
        };

        let manager = StaticPoolManager::new(
            postgres.clone(),
            redis.clone(),
            sui.clone(),
            queue.clone(),
            "static_test_queue".to_string(),
            config,
            topology_ready.clone(),
        );

        manager.discover_and_register().await.unwrap();

        let pool = postgres.get_pool(pool_id).await.unwrap().unwrap();
        assert_eq!(pool.dex_name, "Cetus");
        assert_eq!(pool.liquidity, 50_000_000);
        let active = redis.get_active_pools().await.unwrap().unwrap();
        assert!(active.iter().any(|p| p.pool_id == pool_id));
        assert!(topology_ready.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_no_fallback_when_graphql_fails_on_empty_db() {
        let postgres = Arc::new(InMemoryPostgresStorage::new());
        let redis = Arc::new(InMemoryRedisCache::new());
        let queue = Arc::new(InMemoryMessageQueue::new());
        let sui = Arc::new(MockSuiClient::new());
        let topology_ready = Arc::new(AtomicBool::new(false));

        *sui.query_graphql_with_variables_mock.lock().unwrap() =
            Box::new(|_, _| Err(anyhow!("graphql down")));

        let manager = StaticPoolManager::new(
            postgres.clone(),
            redis,
            sui,
            queue,
            "q".to_string(),
            StaticWorkerConfig::default(),
            topology_ready,
        );

        let err = manager.discover_and_register().await.unwrap_err();
        assert!(err.to_string().contains("graphql down"));
        assert!(postgres.list_pools().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_degraded_mode_preserves_existing_topology() {
        let postgres = Arc::new(InMemoryPostgresStorage::new());
        let existing = PoolState {
            pool_id: "0x_existing".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "0x2::sui::SUI".to_string(),
            coin_type_b: "0xusdc".to_string(),
            sqrt_price: 1,
            liquidity: 1,
            fee_rate: 3000,
            is_paused: false,
        };
        postgres.insert_pool(&existing).await.unwrap();

        let redis = Arc::new(InMemoryRedisCache::new());
        let queue = Arc::new(InMemoryMessageQueue::new());
        let sui = Arc::new(MockSuiClient::new());
        let topology_ready = Arc::new(AtomicBool::new(true));
        *sui.query_graphql_with_variables_mock.lock().unwrap() =
            Box::new(|_, _| Err(anyhow!("graphql down")));

        let manager = StaticPoolManager::new(
            postgres.clone(),
            redis,
            sui,
            queue,
            "q".to_string(),
            StaticWorkerConfig {
                reconciliation_every_n_scans: 0,
                ..StaticWorkerConfig::default()
            },
            topology_ready,
        );

        manager.discover_and_register().await.unwrap();
        assert_eq!(postgres.list_pools().await.unwrap().len(), 1);
    }
}
