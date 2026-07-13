use crate::storage::{ClickhouseAnalyticsTrait, SwapEvent};
use anyhow::{Result, anyhow};
use async_trait::async_trait;

pub struct ClickhouseClient {
    client: clickhouse::Client,
}

impl ClickhouseClient {
    pub fn new(client: clickhouse::Client) -> Self {
        Self { client }
    }

    /// Initializes tables in ClickHouse
    pub async fn create_tables(&self) -> Result<()> {
        self.client
            .query(
                "CREATE TABLE IF NOT EXISTS swap_events (
                    event_id String,
                    timestamp UInt64,
                    pool_id String,
                    dex_name String,
                    sender String,
                    amount_in String,
                    amount_out String,
                    coin_in String,
                    coin_out String
                ) ENGINE = MergeTree()
                ORDER BY (timestamp, pool_id);",
            )
            .execute()
            .await
            .map_err(|e| anyhow!("ClickHouse create table error: {:?}", e))?;
        Ok(())
    }
}

#[async_trait]
impl ClickhouseAnalyticsTrait for ClickhouseClient {
    async fn insert_swap_event(&self, event: &SwapEvent) -> Result<()> {
        let mut inserter = self
            .client
            .insert::<SwapEvent>("swap_events")
            .await
            .map_err(|e| anyhow!("ClickHouse insert init error: {:?}", e))?;

        inserter
            .write(event)
            .await
            .map_err(|e| anyhow!("ClickHouse write row error: {:?}", e))?;

        inserter
            .end()
            .await
            .map_err(|e| anyhow!("ClickHouse insert end error: {:?}", e))?;

        Ok(())
    }

    async fn get_swap_events(&self, limit: u64) -> Result<Vec<SwapEvent>> {
        let events = self
            .client
            .query("SELECT ?fields FROM swap_events ORDER BY timestamp DESC LIMIT ?")
            .bind(limit)
            .fetch_all::<SwapEvent>()
            .await
            .map_err(|e| anyhow!("ClickHouse select query error: {:?}", e))?;

        Ok(events)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::env;

    // Manual mock for testing without active ClickHouse connection
    pub struct InMemoryClickhouseAnalytics {
        pub logs: std::sync::Mutex<Vec<SwapEvent>>,
    }

    impl Default for InMemoryClickhouseAnalytics {
        fn default() -> Self {
            Self::new()
        }
    }

    impl InMemoryClickhouseAnalytics {
        pub fn new() -> Self {
            Self {
                logs: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ClickhouseAnalyticsTrait for InMemoryClickhouseAnalytics {
        async fn insert_swap_event(&self, event: &SwapEvent) -> Result<()> {
            let mut logs = self.logs.lock().unwrap();
            logs.push(event.clone());
            Ok(())
        }

        async fn get_swap_events(&self, limit: u64) -> Result<Vec<SwapEvent>> {
            let logs = self.logs.lock().unwrap();
            let mut sorted = logs.clone();
            sorted.sort_by_key(|b| std::cmp::Reverse(b.timestamp));
            Ok(sorted.into_iter().take(limit as usize).collect())
        }
    }

    #[tokio::test]
    async fn test_in_memory_clickhouse_analytics() {
        let analytics = InMemoryClickhouseAnalytics::new();
        let event = SwapEvent {
            event_id: "tx_1".to_string(),
            timestamp: 1625097600,
            pool_id: "0x_pool_1".to_string(),
            dex_name: "Cetus".to_string(),
            sender: "0xsender".to_string(),
            amount_in: "1000000000".to_string(),
            amount_out: "990000000".to_string(),
            coin_in: "SUI".to_string(),
            coin_out: "USDC".to_string(),
        };

        analytics.insert_swap_event(&event).await.unwrap();
        let fetched = analytics.get_swap_events(5).await.unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].event_id, "tx_1");
    }

    #[tokio::test]
    async fn test_live_clickhouse_if_available() {
        let ch_url = match env::var("CLICKHOUSE_URL") {
            Ok(val) => val,
            Err(_) => return, // skip if not set
        };

        let client = clickhouse::Client::default().with_url(ch_url);
        let ch = ClickhouseClient::new(client);
        ch.create_tables().await.unwrap();

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let event = SwapEvent {
            event_id: format!("live_tx_{}", nanos),
            timestamp: 1718812800,
            pool_id: "0x_live_pool".to_string(),
            dex_name: "Cetus".to_string(),
            sender: "0xsender_live".to_string(),
            amount_in: "5000000000".to_string(),
            amount_out: "4900000000".to_string(),
            coin_in: "SUI".to_string(),
            coin_out: "USDC".to_string(),
        };

        ch.insert_swap_event(&event).await.unwrap();
        let fetched = ch.get_swap_events(10).await.unwrap();
        assert!(!fetched.is_empty());
        assert!(fetched.iter().any(|e| e.event_id == event.event_id));
    }
}
