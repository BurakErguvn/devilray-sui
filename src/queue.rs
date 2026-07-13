use crate::models::{PoolState, PoolTickData};
use crate::storage::SwapEvent;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use redis::AsyncCommands;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum QueueMessage {
    PoolStateUpdate(PoolState),
    PoolTickDataUpdate(PoolTickData),
    SwapEventLog(SwapEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DlqEntry {
    pub message: QueueMessage,
    pub failure_reason: String,
    pub failed_at_unix: u64,
    pub attempts: u32,
}

#[async_trait]
pub trait MessageQueueTrait: Send + Sync {
    /// Publishes a message to the specified queue/list
    async fn publish(&self, queue: &str, msg: &QueueMessage) -> Result<()>;

    /// Consumes/pops a message from the specified queue/list
    async fn consume(&self, queue: &str) -> Result<Option<QueueMessage>>;

    /// Pushes a failed message entry to the dead-letter queue
    async fn push_dlq(&self, dlq: &str, entry: &DlqEntry) -> Result<()>;

    /// Pops the oldest entry from the dead-letter queue
    async fn pop_dlq(&self, dlq: &str) -> Result<Option<DlqEntry>>;

    /// Returns the number of entries in the dead-letter queue
    async fn dlq_len(&self, dlq: &str) -> Result<u64>;
}

pub async fn replay_dlq(
    queue: &dyn MessageQueueTrait,
    main_queue: &str,
    dlq_queue: &str,
    limit: usize,
) -> Result<usize> {
    let mut replayed = 0;
    for _ in 0..limit {
        match queue.pop_dlq(dlq_queue).await? {
            Some(entry) => {
                queue.publish(main_queue, &entry.message).await?;
                replayed += 1;
            }
            None => break,
        }
    }
    Ok(replayed)
}

#[derive(Clone)]
pub struct RedisMessageQueue {
    connection: redis::aio::MultiplexedConnection,
}

impl RedisMessageQueue {
    pub fn new(connection: redis::aio::MultiplexedConnection) -> Self {
        Self { connection }
    }
}

#[async_trait]
impl MessageQueueTrait for RedisMessageQueue {
    async fn publish(&self, queue: &str, msg: &QueueMessage) -> Result<()> {
        let serialized = serde_json::to_string(msg)?;
        let mut conn = self.connection.clone();
        let _: () = conn
            .rpush(queue, serialized)
            .await
            .map_err(|e| anyhow!("Redis rpush error: {:?}", e))?;
        Ok(())
    }

    async fn consume(&self, queue: &str) -> Result<Option<QueueMessage>> {
        let mut conn = self.connection.clone();
        let value: Option<String> = conn
            .lpop(queue, None)
            .await
            .map_err(|e| anyhow!("Redis lpop error: {:?}", e))?;
        if let Some(val) = value {
            let msg: QueueMessage = serde_json::from_str(&val)?;
            Ok(Some(msg))
        } else {
            Ok(None)
        }
    }

    async fn push_dlq(&self, dlq: &str, entry: &DlqEntry) -> Result<()> {
        let serialized = serde_json::to_string(entry)?;
        let mut conn = self.connection.clone();
        let _: () = conn
            .rpush(dlq, serialized)
            .await
            .map_err(|e| anyhow!("Redis dlq rpush error: {:?}", e))?;
        Ok(())
    }

    async fn pop_dlq(&self, dlq: &str) -> Result<Option<DlqEntry>> {
        let mut conn = self.connection.clone();
        let value: Option<String> = conn
            .lpop(dlq, None)
            .await
            .map_err(|e| anyhow!("Redis dlq lpop error: {:?}", e))?;
        if let Some(val) = value {
            let entry: DlqEntry = serde_json::from_str(&val)?;
            Ok(Some(entry))
        } else {
            Ok(None)
        }
    }

    async fn dlq_len(&self, dlq: &str) -> Result<u64> {
        let mut conn = self.connection.clone();
        let len: u64 = conn
            .llen(dlq)
            .await
            .map_err(|e| anyhow!("Redis dlq llen error: {:?}", e))?;
        Ok(len)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::env;
    use std::sync::Mutex;

    pub struct InMemoryMessageQueue {
        pub queue: Mutex<VecDeque<QueueMessage>>,
        pub dlq: Mutex<VecDeque<DlqEntry>>,
    }

    impl Default for InMemoryMessageQueue {
        fn default() -> Self {
            Self::new()
        }
    }

    impl InMemoryMessageQueue {
        pub fn new() -> Self {
            Self {
                queue: Mutex::new(VecDeque::new()),
                dlq: Mutex::new(VecDeque::new()),
            }
        }
    }

    #[async_trait]
    impl MessageQueueTrait for InMemoryMessageQueue {
        async fn publish(&self, _queue: &str, msg: &QueueMessage) -> Result<()> {
            let mut q = self.queue.lock().unwrap();
            q.push_back(msg.clone());
            Ok(())
        }

        async fn consume(&self, _queue: &str) -> Result<Option<QueueMessage>> {
            let mut q = self.queue.lock().unwrap();
            Ok(q.pop_front())
        }

        async fn push_dlq(&self, _dlq: &str, entry: &DlqEntry) -> Result<()> {
            let mut q = self.dlq.lock().unwrap();
            q.push_back(entry.clone());
            Ok(())
        }

        async fn pop_dlq(&self, _dlq: &str) -> Result<Option<DlqEntry>> {
            let mut q = self.dlq.lock().unwrap();
            Ok(q.pop_front())
        }

        async fn dlq_len(&self, _dlq: &str) -> Result<u64> {
            let q = self.dlq.lock().unwrap();
            Ok(q.len() as u64)
        }
    }

    #[tokio::test]
    async fn test_in_memory_queue() {
        let queue = InMemoryMessageQueue::new();
        let msg = QueueMessage::PoolStateUpdate(PoolState {
            pool_id: "0x_test".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "A".to_string(),
            coin_type_b: "B".to_string(),
            sqrt_price: 100,
            liquidity: 200,
            fee_rate: 300,
            is_paused: false,
        });

        queue.publish("test_queue", &msg).await.unwrap();
        let consumed = queue.consume("test_queue").await.unwrap().unwrap();
        assert_eq!(consumed, msg);

        let empty = queue.consume("test_queue").await.unwrap();
        assert!(empty.is_none());
    }

    #[tokio::test]
    async fn test_in_memory_dlq_push_pop_len() {
        let queue = InMemoryMessageQueue::new();
        let msg = QueueMessage::PoolStateUpdate(PoolState {
            pool_id: "0x_dlq".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "A".to_string(),
            coin_type_b: "B".to_string(),
            sqrt_price: 100,
            liquidity: 200,
            fee_rate: 300,
            is_paused: false,
        });

        let entry1 = DlqEntry {
            message: msg.clone(),
            failure_reason: "err1".to_string(),
            failed_at_unix: 1,
            attempts: 3,
        };
        let entry2 = DlqEntry {
            message: msg,
            failure_reason: "err2".to_string(),
            failed_at_unix: 2,
            attempts: 3,
        };

        queue.push_dlq("test_dlq", &entry1).await.unwrap();
        queue.push_dlq("test_dlq", &entry2).await.unwrap();
        assert_eq!(queue.dlq_len("test_dlq").await.unwrap(), 2);

        let popped = queue.pop_dlq("test_dlq").await.unwrap().unwrap();
        assert_eq!(popped.failure_reason, "err1");
        assert_eq!(queue.dlq_len("test_dlq").await.unwrap(), 1);

        let replayed = replay_dlq(&queue, "main_q", "test_dlq", 10).await.unwrap();
        assert_eq!(replayed, 1);
        let consumed = queue.consume("main_q").await.unwrap().unwrap();
        assert_eq!(consumed, popped.message);
        assert_eq!(queue.dlq_len("test_dlq").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_live_redis_queue_if_available() {
        let redis_url = match env::var("REDIS_URL") {
            Ok(val) => val,
            Err(_) => return, // skip if not set
        };

        let client = redis::Client::open(redis_url).unwrap();
        let conn = client.get_multiplexed_async_connection().await.unwrap();
        let queue = RedisMessageQueue::new(conn);

        let msg = QueueMessage::PoolStateUpdate(PoolState {
            pool_id: "0x_test_redis".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "A".to_string(),
            coin_type_b: "B".to_string(),
            sqrt_price: 100,
            liquidity: 200,
            fee_rate: 300,
            is_paused: false,
        });

        let q_name = "test_sui_agg_queue";
        queue.publish(q_name, &msg).await.unwrap();
        let consumed = queue.consume(q_name).await.unwrap().unwrap();
        assert_eq!(consumed, msg);
    }

    #[tokio::test]
    async fn test_live_redis_dlq_if_available() {
        let redis_url = match env::var("REDIS_URL") {
            Ok(val) => val,
            Err(_) => return,
        };

        let client = redis::Client::open(redis_url).unwrap();
        let conn = client.get_multiplexed_async_connection().await.unwrap();
        let queue = RedisMessageQueue::new(conn);

        let msg = QueueMessage::PoolStateUpdate(PoolState {
            pool_id: "0x_dlq_redis".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "A".to_string(),
            coin_type_b: "B".to_string(),
            sqrt_price: 100,
            liquidity: 200,
            fee_rate: 300,
            is_paused: false,
        });
        let entry = DlqEntry {
            message: msg,
            failure_reason: "simulated".to_string(),
            failed_at_unix: 42,
            attempts: 3,
        };

        let dlq_name = "test_sui_agg_dlq";
        queue.push_dlq(dlq_name, &entry).await.unwrap();
        assert_eq!(queue.dlq_len(dlq_name).await.unwrap(), 1);
        let popped = queue.pop_dlq(dlq_name).await.unwrap().unwrap();
        assert_eq!(popped.failure_reason, "simulated");
        assert_eq!(queue.dlq_len(dlq_name).await.unwrap(), 0);
    }
}
