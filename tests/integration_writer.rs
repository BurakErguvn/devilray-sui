#![cfg(feature = "integration")]

use async_trait::async_trait;
use devilray_sui::models::PoolState;
use devilray_sui::queue::{DlqEntry, MessageQueueTrait, QueueMessage, RedisMessageQueue};
use devilray_sui::storage::postgres::PostgresDb;
use devilray_sui::storage::{ClickhouseAnalyticsTrait, PostgresStorageTrait, SwapEvent};
use devilray_sui::workers::db_writer::DatabaseWriter;
use std::env;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;

struct StubClickhouse {
    events: Mutex<Vec<SwapEvent>>,
}

impl StubClickhouse {
    fn new() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl ClickhouseAnalyticsTrait for StubClickhouse {
    async fn insert_swap_event(&self, event: &SwapEvent) -> anyhow::Result<()> {
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }

    async fn get_swap_events(&self, limit: u64) -> anyhow::Result<Vec<SwapEvent>> {
        let events = self.events.lock().unwrap();
        let start = events.len().saturating_sub(limit as usize);
        Ok(events[start..].to_vec())
    }
}

async fn connect_pg() -> Option<PostgresDb> {
    let db_url = env::var("DATABASE_URL").ok()?;
    let pool = sqlx::PgPool::connect(&db_url).await.ok()?;
    let db = PostgresDb::new(pool);
    db.create_tables().await.ok()?;
    Some(db)
}

async fn connect_queue() -> Option<Arc<RedisMessageQueue>> {
    let redis_url = env::var("REDIS_URL").ok()?;
    let client = redis::Client::open(redis_url).ok()?;
    let conn = client.get_multiplexed_async_connection().await.ok()?;
    Some(Arc::new(RedisMessageQueue::new(conn)))
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

#[tokio::test]
async fn writer_e2e_pool_to_pg() {
    let Some(pg) = connect_pg().await else {
        return;
    };
    let Some(queue) = connect_queue().await else {
        return;
    };
    let pg = Arc::new(pg);
    let ch = Arc::new(StubClickhouse::new());
    let queue_trait: Arc<dyn MessageQueueTrait> = queue.clone();

    let suffix = unique_suffix();
    let queue_name = format!("writer_e2e_{suffix}");
    let dlq_name = format!("{queue_name}_dlq");
    let pool_id = format!("0x_writer_e2e_{suffix}");

    let writer = DatabaseWriter::new(pg.clone(), ch, queue_trait.clone(), queue_name.clone());

    let pool = PoolState {
        pool_id: pool_id.clone(),
        dex_name: "Cetus".to_string(),
        coin_type_a: "A".to_string(),
        coin_type_b: "B".to_string(),
        sqrt_price: 42_000,
        liquidity: 1_000_000,
        fee_rate: 2500,
        is_paused: false,
    };
    queue_trait
        .publish(&queue_name, &QueueMessage::PoolStateUpdate(pool))
        .await
        .unwrap();

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let writer_handle = tokio::spawn(async move {
        writer.run(&mut shutdown_rx).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(500)).await;
    shutdown_tx.send(true).unwrap();
    writer_handle.await.unwrap();

    let fetched = pg.get_pool(&pool_id).await.unwrap().unwrap();
    assert_eq!(fetched.sqrt_price, 42_000);
    assert_eq!(queue_trait.dlq_len(&dlq_name).await.unwrap(), 0);
}

#[tokio::test]
async fn writer_replay_dlq_e2e() {
    let Some(pg) = connect_pg().await else {
        return;
    };
    let Some(queue) = connect_queue().await else {
        return;
    };
    let pg = Arc::new(pg);
    let ch = Arc::new(StubClickhouse::new());
    let queue_trait: Arc<dyn MessageQueueTrait> = queue.clone();

    let suffix = unique_suffix();
    let queue_name = format!("writer_replay_{suffix}");
    let dlq_name = format!("{queue_name}_dlq");
    let pool_id = format!("0x_writer_replay_{suffix}");

    let writer = DatabaseWriter::new(pg.clone(), ch, queue_trait.clone(), queue_name.clone());

    let pool = PoolState {
        pool_id: pool_id.clone(),
        dex_name: "Cetus".to_string(),
        coin_type_a: "A".to_string(),
        coin_type_b: "B".to_string(),
        sqrt_price: 88_888,
        liquidity: 500,
        fee_rate: 2500,
        is_paused: false,
    };
    queue_trait
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

    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    let writer_handle = tokio::spawn(async move {
        writer.run(&mut shutdown_rx).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(500)).await;
    shutdown_tx.send(true).unwrap();
    writer_handle.await.unwrap();

    let fetched = pg.get_pool(&pool_id).await.unwrap().unwrap();
    assert_eq!(fetched.sqrt_price, 88_888);
    assert_eq!(queue_trait.dlq_len(&dlq_name).await.unwrap(), 0);
}
