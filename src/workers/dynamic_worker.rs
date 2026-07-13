use crate::collectors::{
    DexDataCollector, cetus::CetusCollector, magma::MagmaCollector, momentum::MomentumCollector,
    turbos::TurbosCollector,
};
use crate::models::PoolState;
use crate::queue::{MessageQueueTrait, QueueMessage};
use crate::storage::{PostgresStorageTrait, RedisCacheTrait, SwapEvent};
use crate::sui_client::SuiClientTrait;
use crate::workers::DynamicWorkerConfig;
use anyhow::Result;
use futures_util::{StreamExt, sink::SinkExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::mpsc::{self, Receiver};
use tokio_tungstenite::tungstenite::Message;

#[derive(Debug, Clone)]
pub struct DynamicTask {
    pub pool_id: String,
    pub dex_name: String,
}

pub struct Collectors {
    pub cetus: CetusCollector,
    pub turbos: TurbosCollector,
    pub magma: MagmaCollector,
    pub momentum: MomentumCollector,
}

impl Default for Collectors {
    fn default() -> Self {
        Self::new()
    }
}

impl Collectors {
    pub fn new() -> Self {
        Self {
            cetus: CetusCollector::new(),
            turbos: TurbosCollector::new(),
            magma: MagmaCollector::new(),
            momentum: MomentumCollector::new(),
        }
    }
}

pub struct DynamicPoolManager {
    postgres_db: Arc<dyn PostgresStorageTrait>,
    redis_cache: Arc<dyn RedisCacheTrait>,
    sui_client: Arc<dyn SuiClientTrait>,
    queue: Arc<dyn MessageQueueTrait>,
    queue_name: String,
    broadcast_tx: broadcast::Sender<PoolState>,
    config: DynamicWorkerConfig,
    collectors: Arc<Collectors>,
}

impl DynamicPoolManager {
    pub fn new(
        postgres_db: Arc<dyn PostgresStorageTrait>,
        redis_cache: Arc<dyn RedisCacheTrait>,
        sui_client: Arc<dyn SuiClientTrait>,
        queue: Arc<dyn MessageQueueTrait>,
        queue_name: String,
        broadcast_tx: broadcast::Sender<PoolState>,
        config: DynamicWorkerConfig,
    ) -> Self {
        Self {
            postgres_db,
            redis_cache,
            sui_client,
            queue,
            queue_name,
            broadcast_tx,
            config,
            collectors: Arc::new(Collectors::new()),
        }
    }

    /// Parses raw WebSocket message and extracts pool_id and matching DEX info if any
    pub fn parse_websocket_message(msg: &str) -> Option<(String, String)> {
        let val: serde_json::Value = serde_json::from_str(msg).ok()?;

        // Parse pool_id from parameters.result.json.pool_id
        let params = val.get("params")?;
        let result = params.get("result")?;
        let type_repr = result.get("type")?.as_str()?;
        let type_lower = type_repr.to_lowercase();
        let dex_name = if type_lower.contains("cetus")
            || type_lower
                .contains("0x1eab094efda6ebb2c11488c02c1c37b80569687e07a3f5c531d0354e33b4e41")
            || type_lower.contains("0x123")
        {
            "Cetus"
        } else if type_lower.contains("turbos")
            || type_lower
                .contains("0x91bfbc386a412c6e3155cc62e307cc9c31405f84d010d8c07656e1bc4028d7b")
        {
            "Turbos"
        } else if type_lower.contains("magma") {
            "Magma"
        } else if type_lower.contains("momentum") {
            "Momentum"
        } else {
            return None;
        };

        let json_fields = result.get("json")?;
        let pool_id = json_fields.get("pool_id")?.as_str()?.to_string();

        Some((pool_id, dex_name.to_string()))
    }

    /// Parses a SwapEvent from raw WebSocket message if it is a swap event
    pub fn parse_swap_event(msg: &str) -> Option<SwapEvent> {
        let val: serde_json::Value = serde_json::from_str(msg).ok()?;
        let params = val.get("params")?;
        let result = params.get("result")?;

        let type_repr = result.get("type")?.as_str()?;
        if !type_repr.contains("SwapEvent") {
            return None;
        }

        let type_lower = type_repr.to_lowercase();
        let dex_name = if type_lower.contains("cetus")
            || type_lower
                .contains("0x1eab094efda6ebb2c11488c02c1c37b80569687e07a3f5c531d0354e33b4e41")
            || type_lower.contains("0x123")
        {
            "Cetus"
        } else if type_lower.contains("turbos")
            || type_lower
                .contains("0x91bfbc386a412c6e3155cc62e307cc9c31405f84d010d8c07656e1bc4028d7b")
        {
            "Turbos"
        } else if type_lower.contains("magma") {
            "Magma"
        } else if type_lower.contains("momentum") {
            "Momentum"
        } else {
            return None;
        };

        let json_fields = result.get("json")?;
        let pool_id = json_fields.get("pool_id")?.as_str()?.to_string();

        let sender = json_fields.get("sender")?.as_str()?.to_string();
        let amount_in = json_fields.get("amount_in")?.as_str()?.to_string();
        let amount_out = json_fields.get("amount_out")?.as_str()?.to_string();
        let coin_in = json_fields.get("coin_in")?.as_str()?.to_string();
        let coin_out = json_fields.get("coin_out")?.as_str()?.to_string();

        let id_sec = result
            .get("id")
            .and_then(|id| id.get("txDigest"))
            .and_then(|d| d.as_str())?
            .to_string();

        let timestamp = result
            .get("timestampMs")
            .and_then(|t| t.as_str())
            .and_then(|t| t.parse::<u64>().ok())
            .map(|t| t / 1000)?;

        Some(SwapEvent {
            event_id: id_sec,
            timestamp,
            pool_id,
            dex_name: dex_name.to_string(),
            sender,
            amount_in,
            amount_out,
            coin_in,
            coin_out,
        })
    }

    /// WebSocket event subscription push listener
    async fn run_websocket_listener(
        websocket_url: String,
        tx: mpsc::Sender<DynamicTask>,
        queue: Arc<dyn MessageQueueTrait>,
        queue_name: String,
        reconnect_interval_ms: u64,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        let mut shutdown = shutdown_rx;

        loop {
            if *shutdown.borrow() {
                break;
            }

            tracing::info!("Connecting to Sui WebSocket endpoint: {}...", websocket_url);

            let ws_stream = match tokio_tungstenite::connect_async(&websocket_url).await {
                Ok((stream, _)) => stream,
                Err(e) => {
                    tracing::error!(
                        "WebSocket connection failed: {:?}. Retrying in {}ms...",
                        e,
                        reconnect_interval_ms
                    );
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(reconnect_interval_ms)) => continue,
                        _ = shutdown.changed() => {
                            if *shutdown.borrow() { break; }
                            continue;
                        }
                    }
                }
            };

            tracing::info!("WebSocket connected successfully! Subscribing to swap events...");
            let (mut ws_tx, mut ws_rx) = ws_stream.split();

            // JSON-RPC subscription message for swap events
            let sub_payload = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "suix_subscribeEvent",
                "params": [
                    {
                        "All": []
                    }
                ]
            });

            if let Err(e) = ws_tx
                .send(Message::Text(sub_payload.to_string().into()))
                .await
            {
                tracing::error!("Failed to send subscription payload: {:?}", e);
                tokio::time::sleep(Duration::from_millis(reconnect_interval_ms)).await;
                continue;
            }

            loop {
                tokio::select! {
                    msg_res = ws_rx.next() => {
                        match msg_res {
                            Some(Ok(Message::Text(text))) => {
                                if let Some((pool_id, dex_name)) = Self::parse_websocket_message(&text) {
                                    tracing::info!("WebSocket push signal: Pool {} updated.", pool_id);
                                    let task = DynamicTask { pool_id, dex_name };
                                    let _ = tx.send(task).await;
                                }

                                if let Some(swap_event) = Self::parse_swap_event(&text) {
                                    tracing::info!("WebSocket parsed SwapEvent: tx={}", swap_event.event_id);
                                    let _ = queue.publish(&queue_name, &QueueMessage::SwapEventLog(swap_event)).await;
                                }
                            }
                            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                                tracing::warn!("WebSocket connection lost. Reconnecting...");
                                break;
                            }
                            _ => {}
                        }
                    }
                    _ = shutdown.changed() => {
                        if *shutdown.borrow() {
                            let _ = ws_tx.send(Message::Close(None)).await;
                            break;
                        }
                    }
                }
            }

            if *shutdown.borrow() {
                break;
            }
        }
        tracing::info!("WebSocket listener task clean exit.");
    }

    /// Circular polling fallback (Pull - backup path)
    async fn run_polling_loop(
        postgres_db: Arc<dyn PostgresStorageTrait>,
        tx: mpsc::Sender<DynamicTask>,
        poll_interval_ms: u64,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        let mut shutdown = shutdown_rx;
        loop {
            if *shutdown.borrow() {
                break;
            }

            // Fallback circular scan
            match postgres_db.list_pools().await {
                Ok(pools) => {
                    tracing::debug!(
                        "Polling fallback check: refreshing {} pools...",
                        pools.len()
                    );
                    for pool in pools {
                        let task = DynamicTask {
                            pool_id: pool.pool_id,
                            dex_name: pool.dex_name,
                        };
                        let _ = tx.send(task).await;
                    }
                }
                Err(err) => {
                    tracing::error!("Polling Postgres list query failed: {:?}", err);
                }
            }

            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(poll_interval_ms)) => {}
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
        tracing::info!("Polling loop task clean exit.");
    }

    /// Runs the dynamic worker pool loop continuously (WebSocket Push + Polling Fallback Pull)
    pub async fn run(&self, shutdown_rx: &mut tokio::sync::watch::Receiver<bool>) -> Result<()> {
        let (tx, rx) = mpsc::channel::<DynamicTask>(self.config.num_workers * 2);
        let shared_rx = Arc::new(tokio::sync::Mutex::new(rx));

        // Spawn dynamic workers
        let mut worker_handles = Vec::new();
        for id in 0..self.config.num_workers {
            let rx_clone = shared_rx.clone();
            let client = self.sui_client.clone();
            let cache = self.redis_cache.clone();
            let queue_clone = self.queue.clone();
            let queue_name_clone = self.queue_name.clone();
            let broadcast_tx_clone = self.broadcast_tx.clone();
            let collectors = self.collectors.clone();
            let config = self.config.clone();

            let handle = tokio::spawn(async move {
                tracing::info!("Dynamic worker {} started.", id);
                Self::worker_loop(
                    rx_clone,
                    client,
                    cache,
                    queue_clone,
                    queue_name_clone,
                    broadcast_tx_clone,
                    collectors,
                    config,
                    id,
                )
                .await;
            });
            worker_handles.push(handle);
        }

        // Spawn WebSocket Event Listener (Push)
        let ws_tx = tx.clone();
        let ws_shutdown_rx = shutdown_rx.clone();
        let ws_url = self.config.websocket_url.clone();
        let ws_reconnect = self.config.ws_reconnect_interval_ms;
        let ws_queue = self.queue.clone();
        let ws_queue_name = self.queue_name.clone();
        let ws_handle = tokio::spawn(async move {
            Self::run_websocket_listener(
                ws_url,
                ws_tx,
                ws_queue,
                ws_queue_name,
                ws_reconnect,
                ws_shutdown_rx,
            )
            .await;
        });

        // Spawn Polling Fallback Loop (Pull)
        let poll_tx = tx.clone();
        let poll_shutdown_rx = shutdown_rx.clone();
        let pg_db = self.postgres_db.clone();
        let poll_interval = self.config.poll_interval_ms;
        let poll_handle = tokio::spawn(async move {
            Self::run_polling_loop(pg_db, poll_tx, poll_interval, poll_shutdown_rx).await;
        });

        // Wait for shutdown trigger
        loop {
            if *shutdown_rx.borrow() {
                break;
            }
            if shutdown_rx.changed().await.is_err() {
                break;
            }
        }

        // Await thread terminations
        ws_handle.await?;
        poll_handle.await?;

        // Drop sender channel to let workers exit
        drop(tx);
        for handle in worker_handles {
            let _ = handle.await;
        }

        tracing::info!("Dynamic Pool Manager clean exit.");
        Ok(())
    }

    /// Single worker loop pulling tasks from the queue channel.
    #[allow(clippy::too_many_arguments)] // worker context bundle; refactor is a separate task
    async fn worker_loop(
        rx: Arc<tokio::sync::Mutex<Receiver<DynamicTask>>>,
        client: Arc<dyn SuiClientTrait>,
        cache: Arc<dyn RedisCacheTrait>,
        queue: Arc<dyn MessageQueueTrait>,
        queue_name: String,
        broadcast_tx: broadcast::Sender<PoolState>,
        collectors: Arc<Collectors>,
        config: DynamicWorkerConfig,
        worker_id: usize,
    ) {
        loop {
            let task_opt = {
                let mut rx_lock = rx.lock().await;
                rx_lock.recv().await
            };

            match task_opt {
                Some(task) => {
                    tracing::debug!("Worker {} processing pool {}", worker_id, task.pool_id);
                    if let Err(err) = Self::process_task(
                        &*client,
                        &*cache,
                        &*queue,
                        &queue_name,
                        &broadcast_tx,
                        &task,
                        &collectors,
                        &config,
                    )
                    .await
                    {
                        tracing::error!(
                            "Worker {} failed to process pool {}: {:?}",
                            worker_id,
                            task.pool_id,
                            err
                        );
                    }
                }
                None => {
                    break;
                }
            }
        }
        tracing::info!("Worker {} finished.", worker_id);
    }

    /// Fetches pool data and updates the hot cache (Redis). Includes retries with exponential backoff.
    #[allow(clippy::too_many_arguments)] // task processing context bundle; refactor is a separate task
    async fn process_task(
        client: &dyn SuiClientTrait,
        cache: &dyn RedisCacheTrait,
        queue: &dyn MessageQueueTrait,
        queue_name: &str,
        broadcast_tx: &broadcast::Sender<PoolState>,
        task: &DynamicTask,
        collectors: &Collectors,
        config: &DynamicWorkerConfig,
    ) -> Result<()> {
        let collector: &dyn DexDataCollector = match task.dex_name.to_lowercase().as_str() {
            "cetus" => &collectors.cetus,
            "turbos" => &collectors.turbos,
            "magma" | "magma finance" => &collectors.magma,
            "momentum" => &collectors.momentum,
            _ => return Err(anyhow::anyhow!("Unsupported DEX: {}", task.dex_name)),
        };

        let mut attempt = 0;
        let mut delay = Duration::from_millis(config.retry_backoff_ms);

        let state = loop {
            match collector.fetch_pool(client, &task.pool_id).await {
                Ok(state) => break state,
                Err(err) => {
                    attempt += 1;
                    if attempt > config.max_retries {
                        return Err(anyhow::anyhow!(
                            "Failed fetching pool state for ID {} after {} attempts. Last error: {:?}",
                            task.pool_id,
                            attempt,
                            err
                        ));
                    }
                    tracing::warn!(
                        "Error fetching pool state for ID {} (attempt {}/{}): {:?}. Retrying in {:?}...",
                        task.pool_id,
                        attempt,
                        config.max_retries,
                        err,
                        delay
                    );
                    tokio::time::sleep(delay).await;
                    delay *= 2; // exponential backoff
                }
            }
        };

        // Cache state in Redis (Hot Path)
        cache.set_pool_state(&state).await?;

        // Queue PoolStateUpdate for async PostgreSQL insertion (Cold Path Buffer)
        queue
            .publish(queue_name, &QueueMessage::PoolStateUpdate(state.clone()))
            .await?;

        // Broadcast pool state update to public WebSocket clients
        let _ = broadcast_tx.send(state);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PoolState;
    use crate::queue::tests::InMemoryMessageQueue;
    use crate::storage::postgres::tests::InMemoryPostgresStorage;
    use crate::storage::redis::tests::InMemoryRedisCache;
    use crate::sui_client::tests::MockSuiClient;
    use serde_json::json;
    use tokio::sync::watch;

    #[tokio::test]
    async fn test_dynamic_worker_pool() {
        let postgres = Arc::new(InMemoryPostgresStorage::new());
        let redis = Arc::new(InMemoryRedisCache::new());
        let sui = Arc::new(MockSuiClient::new());
        let queue = Arc::new(InMemoryMessageQueue::new());
        let (broadcast_tx, _) = broadcast::channel(10);

        // Insert an active pool in Postgres
        let pool = PoolState {
            pool_id: "0x_dynamic_test_pool".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "0xcoinA".to_string(),
            coin_type_b: "0xcoinB".to_string(),
            sqrt_price: 18446744073709551616,
            liquidity: 1000000,
            fee_rate: 3000,
            is_paused: false,
        };
        postgres.insert_pool(&pool).await.unwrap();

        // Mock Sui RPC return data
        *sui.get_object_mock.lock().unwrap() = Box::new(|pool_id| {
            assert_eq!(pool_id, "0x_dynamic_test_pool");
            Ok(json!({
                "data": {
                    "objectId": "0x_dynamic_test_pool",
                    "content": {
                        "type": "0x1eabed::pool::Pool<0x2::sui::SUI, 0x5d4b3::coin::COIN>",
                        "fields": {
                            "current_sqrt_price": "18446744073709551616",
                            "liquidity": "1000000",
                            "is_pause": false,
                            "fee_rate": "3000"
                        }
                    }
                }
            }))
        });

        // Config setup
        let config = DynamicWorkerConfig {
            num_workers: 2,
            poll_interval_ms: 100,
            max_retries: 2,
            retry_backoff_ms: 10,
            websocket_url: "ws://localhost:9999".to_string(),
            ws_reconnect_interval_ms: 10,
        };

        let manager = DynamicPoolManager::new(
            postgres.clone(),
            redis.clone(),
            sui.clone(),
            queue.clone(),
            "test_dynamic_queue".to_string(),
            broadcast_tx.clone(),
            config,
        );

        // Spawn manager with a shutdown watch channel
        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let manager_handle = tokio::spawn(async move {
            manager.run(&mut shutdown_rx).await.unwrap();
        });

        // Wait a short time to process the loop
        tokio::time::sleep(Duration::from_millis(250)).await;

        // Verify state is cached in Redis
        let cached = redis
            .get_pool_state("0x_dynamic_test_pool")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cached.sqrt_price, 18446744073709551616);
        assert_eq!(cached.liquidity, 1000000);

        // Verify state was also published to the Message Queue
        let mq_msg = queue.consume("test_dynamic_queue").await.unwrap().unwrap();
        match mq_msg {
            crate::queue::QueueMessage::PoolStateUpdate(state) => {
                assert_eq!(state.pool_id, "0x_dynamic_test_pool");
                assert_eq!(state.sqrt_price, 18446744073709551616);
            }
            _ => panic!("Expected PoolStateUpdate queue message"),
        }

        // Trigger shutdown
        shutdown_tx.send(true).unwrap();
        manager_handle.await.unwrap();
    }

    #[test]
    fn test_parse_websocket_message() {
        let msg = r#"{
            "jsonrpc": "2.0",
            "method": "suix_subscribeEvent",
            "params": {
                "subscription": 123,
                "result": {
                    "type": "0x123::pool::SwapEvent<0x2::sui::SUI>",
                    "json": {
                        "pool_id": "0x_ws_pool_id_123"
                    }
                }
            }
        }"#;

        let parsed = DynamicPoolManager::parse_websocket_message(msg).unwrap();
        assert_eq!(parsed.0, "0x_ws_pool_id_123");
        assert_eq!(parsed.1, "Cetus");
    }

    #[test]
    fn test_parse_swap_event() {
        let msg = r#"{
            "jsonrpc": "2.0",
            "method": "suix_subscribeEvent",
            "params": {
                "subscription": 123,
                "result": {
                    "type": "0x123::pool::SwapEvent<0x2::sui::SUI>",
                    "json": {
                        "pool_id": "0x_ws_pool_id_123",
                        "sender": "0x_user_alice",
                        "amount_in": "5000",
                        "amount_out": "4990",
                        "coin_in": "0x2::sui::SUI",
                        "coin_out": "0x5d4b3::coin::USDC"
                    },
                    "id": {
                        "txDigest": "0x_some_tx_digest"
                    },
                    "timestampMs": "1710000000000"
                }
            }
        }"#;

        let event = DynamicPoolManager::parse_swap_event(msg).unwrap();
        assert_eq!(event.pool_id, "0x_ws_pool_id_123");
        assert_eq!(event.dex_name, "Cetus");
        assert_eq!(event.sender, "0x_user_alice");
        assert_eq!(event.amount_in, "5000");
        assert_eq!(event.amount_out, "4990");
        assert_eq!(event.coin_in, "0x2::sui::SUI");
        assert_eq!(event.coin_out, "0x5d4b3::coin::USDC");
        assert_eq!(event.event_id, "0x_some_tx_digest");
        assert_eq!(event.timestamp, 1710000000);
    }
}
