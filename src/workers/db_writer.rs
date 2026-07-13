use crate::api::metrics;
use crate::queue::{DlqEntry, MessageQueueTrait, QueueMessage};
use crate::storage::{ClickhouseAnalyticsTrait, PostgresStorageTrait};
use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;

pub struct DatabaseWriter {
    postgres_db: Arc<dyn PostgresStorageTrait>,
    clickhouse_db: Arc<dyn ClickhouseAnalyticsTrait>,
    queue: Arc<dyn MessageQueueTrait>,
    queue_name: String,
    dlq_name: String,
}

impl DatabaseWriter {
    pub fn new(
        postgres_db: Arc<dyn PostgresStorageTrait>,
        clickhouse_db: Arc<dyn ClickhouseAnalyticsTrait>,
        queue: Arc<dyn MessageQueueTrait>,
        queue_name: String,
    ) -> Self {
        let dlq_name = format!("{queue_name}_dlq");
        Self {
            postgres_db,
            clickhouse_db,
            queue,
            queue_name,
            dlq_name,
        }
    }

    pub async fn replay_dlq(&self, limit: usize) -> Result<usize> {
        crate::queue::replay_dlq(self.queue.as_ref(), &self.queue_name, &self.dlq_name, limit).await
    }

    async fn persist_with_retry<F, Fut>(&self, msg: &QueueMessage, persist: F)
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
    {
        let mut attempts = 0u32;
        let max_attempts = 3;

        loop {
            match persist().await {
                Ok(_) => return,
                Err(e) => {
                    attempts += 1;
                    tracing::error!(
                        "persist failed (attempt {}/{max_attempts}): {:?}",
                        attempts,
                        e
                    );
                    if attempts >= max_attempts {
                        let entry = DlqEntry {
                            message: msg.clone(),
                            failure_reason: format!("{e:?}"),
                            failed_at_unix: SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap()
                                .as_secs(),
                            attempts,
                        };
                        if let Err(d_err) = self.queue.push_dlq(&self.dlq_name, &entry).await {
                            tracing::error!("FAILED to push to DLQ {}: {:?}", self.dlq_name, d_err);
                        } else {
                            metrics::record_dlq_push();
                        }
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(200 * attempts as u64)).await;
                }
            }
        }
    }

    pub async fn run(&self, shutdown_rx: &mut watch::Receiver<bool>) -> Result<()> {
        tracing::info!(
            "DatabaseWriter consumer loop started. Reading queue: {}",
            self.queue_name
        );
        loop {
            if *shutdown_rx.borrow() {
                break;
            }

            match self.queue.consume(&self.queue_name).await {
                Ok(Some(msg)) => match &msg {
                    QueueMessage::PoolStateUpdate(pool_state) => {
                        tracing::debug!(
                            "DatabaseWriter consuming PoolStateUpdate for pool {}",
                            pool_state.pool_id
                        );
                        let pool_state = pool_state.clone();
                        self.persist_with_retry(&msg, || async {
                            self.postgres_db.insert_pool(&pool_state).await
                        })
                        .await;
                    }
                    QueueMessage::PoolTickDataUpdate(tick_data) => {
                        tracing::debug!(
                            "DatabaseWriter consuming PoolTickDataUpdate for pool {}",
                            tick_data.pool_id
                        );
                        let tick_data = tick_data.clone();
                        self.persist_with_retry(&msg, || async {
                            self.postgres_db.set_pool_tick_data(&tick_data).await
                        })
                        .await;
                    }
                    QueueMessage::SwapEventLog(swap_event) => {
                        tracing::debug!(
                            "DatabaseWriter consuming SwapEventLog for tx {}",
                            swap_event.event_id
                        );
                        let swap_event = swap_event.clone();
                        self.persist_with_retry(&msg, || async {
                            self.clickhouse_db.insert_swap_event(&swap_event).await
                        })
                        .await;
                    }
                },
                Ok(None) => {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Error consuming message from queue: {:?}", e);
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                break;
                            }
                        }
                    }
                }
            }
        }
        tracing::info!("DatabaseWriter consumer loop terminated.");
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::models::{PoolState, PoolTickData, TickInfo};
    use crate::queue::tests::InMemoryMessageQueue;
    use crate::storage::SwapEvent;
    use crate::storage::clickhouse_storage::tests::InMemoryClickhouseAnalytics;
    use crate::storage::postgres::tests::InMemoryPostgresStorage;
    use async_trait::async_trait;

    pub struct FailingPostgresStorage {
        inner: InMemoryPostgresStorage,
    }

    impl Default for FailingPostgresStorage {
        fn default() -> Self {
            Self::new()
        }
    }

    impl FailingPostgresStorage {
        pub fn new() -> Self {
            Self {
                inner: InMemoryPostgresStorage::new(),
            }
        }
    }

    #[async_trait]
    impl PostgresStorageTrait for FailingPostgresStorage {
        async fn insert_token(&self, token: &crate::models::Token) -> Result<()> {
            self.inner.insert_token(token).await
        }

        async fn get_token(&self, address: &str) -> Result<Option<crate::models::Token>> {
            self.inner.get_token(address).await
        }

        async fn list_tokens(&self) -> Result<Vec<crate::models::Token>> {
            self.inner.list_tokens().await
        }

        async fn insert_pool(&self, _pool: &PoolState) -> Result<()> {
            Err(anyhow::anyhow!("simulated failure"))
        }

        async fn get_pool(&self, pool_id: &str) -> Result<Option<PoolState>> {
            self.inner.get_pool(pool_id).await
        }

        async fn list_pools(&self) -> Result<Vec<PoolState>> {
            self.inner.list_pools().await
        }

        async fn set_reference_gas_price(&self, price: u64) -> Result<()> {
            self.inner.set_reference_gas_price(price).await
        }

        async fn get_reference_gas_price(&self) -> Result<Option<u64>> {
            self.inner.get_reference_gas_price().await
        }

        async fn set_pool_tick_data(&self, data: &PoolTickData) -> Result<()> {
            self.inner.set_pool_tick_data(data).await
        }

        async fn get_pool_tick_data(&self, pool_id: &str) -> Result<Option<PoolTickData>> {
            self.inner.get_pool_tick_data(pool_id).await
        }

        async fn get_pool_discovery_progress(
            &self,
            dex_name: &str,
            event_type: &str,
        ) -> Result<Option<crate::discovery::progress::PoolDiscoveryProgress>> {
            self.inner
                .get_pool_discovery_progress(dex_name, event_type)
                .await
        }

        async fn commit_discovery_page(
            &self,
            commit: &crate::discovery::progress::DiscoveryPageCommit,
        ) -> Result<()> {
            self.inner.commit_discovery_page(commit).await
        }

        async fn resolve_discovery_failures(
            &self,
            dex_name: &str,
            event_ids: &[String],
        ) -> Result<()> {
            self.inner
                .resolve_discovery_failures(dex_name, event_ids)
                .await
        }

        async fn list_pool_discovery_failures(
            &self,
            dex_name: &str,
            limit: u32,
        ) -> Result<Vec<crate::discovery::progress::PoolDiscoveryFailure>> {
            self.inner
                .list_pool_discovery_failures(dex_name, limit)
                .await
        }
    }

    #[tokio::test]
    async fn test_database_writer_processing() {
        let pg = Arc::new(InMemoryPostgresStorage::new());
        let ch = Arc::new(InMemoryClickhouseAnalytics::new());
        let queue = Arc::new(InMemoryMessageQueue::new());

        let writer = DatabaseWriter::new(
            pg.clone(),
            ch.clone(),
            queue.clone(),
            "test_db_writer_queue".to_string(),
        );

        let pool = PoolState {
            pool_id: "0x_test_writer_pool".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "A".to_string(),
            coin_type_b: "B".to_string(),
            sqrt_price: 100,
            liquidity: 200,
            fee_rate: 300,
            is_paused: false,
        };
        queue
            .publish(
                "test_db_writer_queue",
                &QueueMessage::PoolStateUpdate(pool.clone()),
            )
            .await
            .unwrap();

        let event = SwapEvent {
            event_id: "0x_test_event_id".to_string(),
            timestamp: 1_710_000_000,
            pool_id: "0x_test_writer_pool".to_string(),
            dex_name: "Cetus".to_string(),
            sender: "0x_user".to_string(),
            amount_in: "100".to_string(),
            amount_out: "99".to_string(),
            coin_in: "A".to_string(),
            coin_out: "B".to_string(),
        };
        queue
            .publish(
                "test_db_writer_queue",
                &QueueMessage::SwapEventLog(event.clone()),
            )
            .await
            .unwrap();

        let tick_data = PoolTickData {
            pool_id: "0x_test_writer_pool".to_string(),
            current_tick_index: 42,
            tick_spacing: 60,
            ticks: vec![TickInfo {
                tick_index: 60,
                liquidity_net: 1000,
            }],
            ..Default::default()
        };
        queue
            .publish(
                "test_db_writer_queue",
                &QueueMessage::PoolTickDataUpdate(tick_data.clone()),
            )
            .await
            .unwrap();

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let writer_handle = tokio::spawn(async move {
            writer.run(&mut shutdown_rx).await.unwrap();
        });

        tokio::time::sleep(Duration::from_millis(50)).await;

        let fetched_pool = pg.get_pool("0x_test_writer_pool").await.unwrap().unwrap();
        assert_eq!(fetched_pool.sqrt_price, 100);

        let fetched_ticks = pg
            .get_pool_tick_data("0x_test_writer_pool")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched_ticks.current_tick_index, 42);

        let events = ch.get_swap_events(10).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].amount_in, "100");

        assert_eq!(queue.dlq_len("test_db_writer_queue_dlq").await.unwrap(), 0);

        shutdown_tx.send(true).unwrap();
        writer_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_db_writer_routes_to_dlq_on_persistent_failure() {
        let pg = Arc::new(FailingPostgresStorage::new());
        let ch = Arc::new(InMemoryClickhouseAnalytics::new());
        let queue = Arc::new(InMemoryMessageQueue::new());
        let dlq_name = "test_fail_queue_dlq";

        let writer = DatabaseWriter::new(pg, ch, queue.clone(), "test_fail_queue".to_string());

        let pool = PoolState {
            pool_id: "0x_fail_pool".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "A".to_string(),
            coin_type_b: "B".to_string(),
            sqrt_price: 100,
            liquidity: 200,
            fee_rate: 300,
            is_paused: false,
        };
        queue
            .publish("test_fail_queue", &QueueMessage::PoolStateUpdate(pool))
            .await
            .unwrap();

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
        let writer_handle = tokio::spawn(async move {
            writer.run(&mut shutdown_rx).await.unwrap();
        });

        tokio::time::sleep(Duration::from_millis(800)).await;

        let entry = queue.pop_dlq(dlq_name).await.unwrap().unwrap();
        assert_eq!(entry.attempts, 3);
        assert!(!entry.failure_reason.is_empty());
        assert!(queue.consume("test_fail_queue").await.unwrap().is_none());
        assert!(queue.pop_dlq(dlq_name).await.unwrap().is_none());

        shutdown_tx.send(true).unwrap();
        writer_handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_db_writer_replay_dlq() {
        let pg = Arc::new(InMemoryPostgresStorage::new());
        let ch = Arc::new(InMemoryClickhouseAnalytics::new());
        let queue = Arc::new(InMemoryMessageQueue::new());
        let queue_name = "test_replay_queue".to_string();
        let dlq_name = format!("{queue_name}_dlq");

        let writer = DatabaseWriter::new(pg, ch, queue.clone(), queue_name.clone());

        let pool = PoolState {
            pool_id: "0x_replay_pool".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "A".to_string(),
            coin_type_b: "B".to_string(),
            sqrt_price: 100,
            liquidity: 200,
            fee_rate: 300,
            is_paused: false,
        };
        queue
            .push_dlq(
                &dlq_name,
                &DlqEntry {
                    message: QueueMessage::PoolStateUpdate(pool),
                    failure_reason: "manual".to_string(),
                    failed_at_unix: 1,
                    attempts: 3,
                },
            )
            .await
            .unwrap();

        let replayed = writer.replay_dlq(1).await.unwrap();
        assert_eq!(replayed, 1);
        assert!(queue.consume(&queue_name).await.unwrap().is_some());
        assert_eq!(queue.dlq_len(&dlq_name).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_dlq_metric_family_present() {
        metrics::record_dlq_push();
        let body = metrics::metrics_body_for_test();
        assert!(body.contains("dlq_pushed_total"));
    }
}
