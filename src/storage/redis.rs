use crate::models::{PoolState, PoolTickData, Token};
use crate::storage::RedisCacheTrait;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use redis::AsyncCommands;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Clone)]
pub struct RedisCache {
    client: redis::Client,
    connection: Arc<RwLock<redis::aio::MultiplexedConnection>>,
}

impl RedisCache {
    pub fn new(client: redis::Client, connection: redis::aio::MultiplexedConnection) -> Self {
        Self {
            client,
            connection: Arc::new(RwLock::new(connection)),
        }
    }

    async fn get_connection(&self) -> redis::aio::MultiplexedConnection {
        let conn = self.connection.read().await;
        conn.clone()
    }

    async fn reconnect(&self) -> Result<redis::aio::MultiplexedConnection> {
        let mut conn_write = self.connection.write().await;
        match self.client.get_multiplexed_async_connection().await {
            Ok(new_conn) => {
                *conn_write = new_conn.clone();
                Ok(new_conn)
            }
            Err(e) => Err(anyhow!("Failed to reconnect to Redis: {:?}", e)),
        }
    }
}

#[async_trait]
impl RedisCacheTrait for RedisCache {
    async fn set_pool_state(&self, state: &PoolState) -> Result<()> {
        let key = format!("pool:{}", state.pool_id);
        let serialized = serde_json::to_string(state)?;

        let mut attempts = 0;
        let max_attempts = 3;

        loop {
            let mut conn = self.get_connection().await;
            match conn.set_ex::<_, _, ()>(&key, &serialized, 300).await {
                Ok(_) => return Ok(()),
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_attempts {
                        return Err(anyhow!("Redis set error after retry: {:?}", e));
                    }
                    if let Err(reconn_err) = self.reconnect().await {
                        tracing::warn!("Redis reconnection failed: {:?}", reconn_err);
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(100 * attempts)).await;
                }
            }
        }
    }

    async fn get_pool_state(&self, pool_id: &str) -> Result<Option<PoolState>> {
        let key = format!("pool:{}", pool_id);

        let mut attempts = 0;
        let max_attempts = 3;

        loop {
            let mut conn = self.get_connection().await;
            match conn.get::<_, Option<String>>(&key).await {
                Ok(value) => {
                    if let Some(val) = value {
                        let state: PoolState = serde_json::from_str(&val)?;
                        return Ok(Some(state));
                    } else {
                        return Ok(None);
                    }
                }
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_attempts {
                        return Err(anyhow!("Redis get error after retry: {:?}", e));
                    }
                    if let Err(reconn_err) = self.reconnect().await {
                        tracing::warn!("Redis reconnection failed: {:?}", reconn_err);
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(100 * attempts)).await;
                }
            }
        }
    }

    async fn set_reference_gas_price(&self, price: u64) -> Result<()> {
        let key = "config:reference_gas_price";

        let mut attempts = 0;
        let max_attempts = 3;

        loop {
            let mut conn = self.get_connection().await;
            match conn.set_ex::<_, _, ()>(key, price, 3600).await {
                Ok(_) => return Ok(()),
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_attempts {
                        return Err(anyhow!("Redis set gas price error after retry: {:?}", e));
                    }
                    if let Err(reconn_err) = self.reconnect().await {
                        tracing::warn!("Redis reconnection failed: {:?}", reconn_err);
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(100 * attempts)).await;
                }
            }
        }
    }

    async fn get_reference_gas_price(&self) -> Result<Option<u64>> {
        let key = "config:reference_gas_price";

        let mut attempts = 0;
        let max_attempts = 3;

        loop {
            let mut conn = self.get_connection().await;
            match conn.get::<_, Option<u64>>(key).await {
                Ok(value) => return Ok(value),
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_attempts {
                        return Err(anyhow!("Redis get gas price error after retry: {:?}", e));
                    }
                    if let Err(reconn_err) = self.reconnect().await {
                        tracing::warn!("Redis reconnection failed: {:?}", reconn_err);
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(100 * attempts)).await;
                }
            }
        }
    }

    async fn set_pool_tick_data(&self, data: &PoolTickData) -> Result<()> {
        let key = format!("pool_ticks:{}", data.pool_id);
        let serialized = serde_json::to_string(data)?;

        let mut attempts = 0;
        let max_attempts = 3;

        loop {
            let mut conn = self.get_connection().await;
            match conn.set_ex::<_, _, ()>(&key, &serialized, 300).await {
                Ok(_) => return Ok(()),
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_attempts {
                        return Err(anyhow!(
                            "Redis set pool tick data error after retry: {:?}",
                            e
                        ));
                    }
                    if let Err(reconn_err) = self.reconnect().await {
                        tracing::warn!("Redis reconnection failed: {:?}", reconn_err);
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(100 * attempts)).await;
                }
            }
        }
    }

    async fn get_pool_tick_data(&self, pool_id: &str) -> Result<Option<PoolTickData>> {
        let key = format!("pool_ticks:{}", pool_id);

        let mut attempts = 0;
        let max_attempts = 3;

        loop {
            let mut conn = self.get_connection().await;
            match conn.get::<_, Option<String>>(&key).await {
                Ok(value) => {
                    if let Some(val) = value {
                        let data: PoolTickData = serde_json::from_str(&val)?;
                        return Ok(Some(data));
                    } else {
                        return Ok(None);
                    }
                }
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_attempts {
                        return Err(anyhow!(
                            "Redis get pool tick data error after retry: {:?}",
                            e
                        ));
                    }
                    if let Err(reconn_err) = self.reconnect().await {
                        tracing::warn!("Redis reconnection failed: {:?}", reconn_err);
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(100 * attempts)).await;
                }
            }
        }
    }

    async fn set_active_pools(&self, pools: &[PoolState]) -> Result<()> {
        let key = "pools:active";
        let serialized = serde_json::to_string(pools)?;

        let mut attempts = 0;
        let max_attempts = 3;

        loop {
            let mut conn = self.get_connection().await;
            match conn.set_ex::<_, _, ()>(key, &serialized, 30).await {
                Ok(_) => return Ok(()),
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_attempts {
                        return Err(anyhow!("Redis set active pools error after retry: {:?}", e));
                    }
                    if let Err(reconn_err) = self.reconnect().await {
                        tracing::warn!("Redis reconnection failed: {:?}", reconn_err);
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(100 * attempts)).await;
                }
            }
        }
    }

    async fn get_active_pools(&self) -> Result<Option<Vec<PoolState>>> {
        let key = "pools:active";

        let mut attempts = 0;
        let max_attempts = 3;

        loop {
            let mut conn = self.get_connection().await;
            match conn.get::<_, Option<String>>(key).await {
                Ok(value) => {
                    if let Some(val) = value {
                        let pools: Vec<PoolState> = serde_json::from_str(&val)?;
                        return Ok(Some(pools));
                    } else {
                        return Ok(None);
                    }
                }
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_attempts {
                        return Err(anyhow!("Redis get active pools error after retry: {:?}", e));
                    }
                    if let Err(reconn_err) = self.reconnect().await {
                        tracing::warn!("Redis reconnection failed: {:?}", reconn_err);
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(100 * attempts)).await;
                }
            }
        }
    }

    async fn set_all_tokens(&self, tokens: &[Token]) -> Result<()> {
        let key = "tokens:all";
        let serialized = serde_json::to_string(tokens)?;

        let mut attempts = 0;
        let max_attempts = 3;

        loop {
            let mut conn = self.get_connection().await;
            match conn.set_ex::<_, _, ()>(key, &serialized, 300).await {
                Ok(_) => return Ok(()),
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_attempts {
                        return Err(anyhow!("Redis set all tokens error after retry: {:?}", e));
                    }
                    if let Err(reconn_err) = self.reconnect().await {
                        tracing::warn!("Redis reconnection failed: {:?}", reconn_err);
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(100 * attempts)).await;
                }
            }
        }
    }

    async fn get_all_tokens(&self) -> Result<Option<Vec<Token>>> {
        let key = "tokens:all";

        let mut attempts = 0;
        let max_attempts = 3;

        loop {
            let mut conn = self.get_connection().await;
            match conn.get::<_, Option<String>>(key).await {
                Ok(value) => {
                    if let Some(val) = value {
                        let tokens: Vec<Token> = serde_json::from_str(&val)?;
                        return Ok(Some(tokens));
                    } else {
                        return Ok(None);
                    }
                }
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_attempts {
                        return Err(anyhow!("Redis get all tokens error after retry: {:?}", e));
                    }
                    if let Err(reconn_err) = self.reconnect().await {
                        tracing::warn!("Redis reconnection failed: {:?}", reconn_err);
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(100 * attempts)).await;
                }
            }
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::env;

    // Manual mock for testing without active redis connection
    pub struct InMemoryRedisCache {
        pub cache: std::sync::Mutex<std::collections::HashMap<String, String>>,
    }

    impl Default for InMemoryRedisCache {
        fn default() -> Self {
            Self::new()
        }
    }

    impl InMemoryRedisCache {
        pub fn new() -> Self {
            Self {
                cache: std::sync::Mutex::new(std::collections::HashMap::new()),
            }
        }
    }

    #[async_trait]
    impl RedisCacheTrait for InMemoryRedisCache {
        async fn set_pool_state(&self, state: &PoolState) -> Result<()> {
            let mut cache = self.cache.lock().unwrap();
            let key = format!("pool:{}", state.pool_id);
            let serialized = serde_json::to_string(state)?;
            cache.insert(key, serialized);
            Ok(())
        }

        async fn get_pool_state(&self, pool_id: &str) -> Result<Option<PoolState>> {
            let cache = self.cache.lock().unwrap();
            let key = format!("pool:{}", pool_id);
            if let Some(val) = cache.get(&key) {
                let state: PoolState = serde_json::from_str(val)?;
                Ok(Some(state))
            } else {
                Ok(None)
            }
        }

        async fn set_reference_gas_price(&self, price: u64) -> Result<()> {
            let mut cache = self.cache.lock().unwrap();
            cache.insert("config:reference_gas_price".to_string(), price.to_string());
            Ok(())
        }

        async fn get_reference_gas_price(&self) -> Result<Option<u64>> {
            let cache = self.cache.lock().unwrap();
            if let Some(val) = cache.get("config:reference_gas_price") {
                let price = val.parse::<u64>()?;
                Ok(Some(price))
            } else {
                Ok(None)
            }
        }

        async fn set_pool_tick_data(&self, data: &PoolTickData) -> Result<()> {
            let mut cache = self.cache.lock().unwrap();
            let key = format!("pool_ticks:{}", data.pool_id);
            let serialized = serde_json::to_string(data)?;
            cache.insert(key, serialized);
            Ok(())
        }

        async fn get_pool_tick_data(&self, pool_id: &str) -> Result<Option<PoolTickData>> {
            let cache = self.cache.lock().unwrap();
            let key = format!("pool_ticks:{}", pool_id);
            if let Some(val) = cache.get(&key) {
                let data: PoolTickData = serde_json::from_str(val)?;
                Ok(Some(data))
            } else {
                Ok(None)
            }
        }

        async fn set_active_pools(&self, pools: &[PoolState]) -> Result<()> {
            let mut cache = self.cache.lock().unwrap();
            let serialized = serde_json::to_string(pools)?;
            cache.insert("pools:active".to_string(), serialized);
            Ok(())
        }

        async fn get_active_pools(&self) -> Result<Option<Vec<PoolState>>> {
            let cache = self.cache.lock().unwrap();
            if let Some(val) = cache.get("pools:active") {
                let pools: Vec<PoolState> = serde_json::from_str(val)?;
                Ok(Some(pools))
            } else {
                Ok(None)
            }
        }

        async fn set_all_tokens(&self, tokens: &[Token]) -> Result<()> {
            let mut cache = self.cache.lock().unwrap();
            let serialized = serde_json::to_string(tokens)?;
            cache.insert("tokens:all".to_string(), serialized);
            Ok(())
        }

        async fn get_all_tokens(&self) -> Result<Option<Vec<Token>>> {
            let cache = self.cache.lock().unwrap();
            if let Some(val) = cache.get("tokens:all") {
                let tokens: Vec<Token> = serde_json::from_str(val)?;
                Ok(Some(tokens))
            } else {
                Ok(None)
            }
        }
    }

    #[tokio::test]
    async fn test_in_memory_active_pools_cache() {
        let cache = InMemoryRedisCache::new();
        let pools = vec![PoolState {
            pool_id: "0x_active".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "SUI".to_string(),
            coin_type_b: "USDC".to_string(),
            sqrt_price: 1000,
            liquidity: 500,
            fee_rate: 3000,
            is_paused: false,
        }];
        cache.set_active_pools(&pools).await.unwrap();
        let fetched = cache.get_active_pools().await.unwrap().unwrap();
        assert_eq!(fetched, pools);
    }

    #[tokio::test]
    async fn test_in_memory_all_tokens_cache() {
        use crate::models::Token;
        let cache = InMemoryRedisCache::new();
        let tokens = vec![Token {
            address: "0x2::sui::SUI".to_string(),
            symbol: "SUI".to_string(),
            name: "Sui".to_string(),
            decimals: 9,
        }];
        cache.set_all_tokens(&tokens).await.unwrap();
        let fetched = cache.get_all_tokens().await.unwrap().unwrap();
        assert_eq!(fetched, tokens);
    }

    #[tokio::test]
    async fn test_live_active_pools_if_available() {
        let redis_url = match env::var("REDIS_URL") {
            Ok(val) => val,
            Err(_) => return,
        };

        let client = redis::Client::open(redis_url).unwrap();
        let conn = client.get_multiplexed_async_connection().await.unwrap();
        let cache = RedisCache::new(client.clone(), conn);

        let pools = vec![PoolState {
            pool_id: "0x_live_active".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "A".to_string(),
            coin_type_b: "B".to_string(),
            sqrt_price: 42,
            liquidity: 99,
            fee_rate: 2500,
            is_paused: false,
        }];
        cache.set_active_pools(&pools).await.unwrap();
        let fetched = cache.get_active_pools().await.unwrap().unwrap();
        assert_eq!(fetched, pools);
    }

    #[tokio::test]
    async fn test_in_memory_pool_tick_data_cache() {
        let cache = InMemoryRedisCache::new();
        let data = PoolTickData {
            pool_id: "0x_redis_ticks".to_string(),
            current_tick_index: 10,
            tick_spacing: 60,
            ticks: vec![crate::models::TickInfo {
                tick_index: 60,
                liquidity_net: 5000,
            }],
            ..Default::default()
        };
        cache.set_pool_tick_data(&data).await.unwrap();
        let fetched = cache
            .get_pool_tick_data("0x_redis_ticks")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched, data);
    }

    #[tokio::test]
    async fn test_in_memory_redis_cache() {
        let cache = InMemoryRedisCache::new();
        let pool = PoolState {
            pool_id: "0x_redis_test".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "0xcoinA".to_string(),
            coin_type_b: "0xcoinB".to_string(),
            sqrt_price: 1000,
            liquidity: 500,
            fee_rate: 3000,
            is_paused: false,
        };

        cache.set_pool_state(&pool).await.unwrap();
        let fetched = cache
            .get_pool_state("0x_redis_test")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.sqrt_price, 1000);
    }

    #[tokio::test]
    async fn test_live_redis_if_available() {
        let redis_url = match env::var("REDIS_URL") {
            Ok(val) => val,
            Err(_) => return, // skip if not set
        };

        let client = redis::Client::open(redis_url).unwrap();
        let conn = client.get_multiplexed_async_connection().await.unwrap();
        let cache = RedisCache::new(client.clone(), conn);

        let pool = PoolState {
            pool_id: "0x_live_redis_test".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "0xcoinA".to_string(),
            coin_type_b: "0xcoinB".to_string(),
            sqrt_price: 99999,
            liquidity: 88888,
            fee_rate: 2500,
            is_paused: false,
        };

        cache.set_pool_state(&pool).await.unwrap();
        let fetched = cache
            .get_pool_state("0x_live_redis_test")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.sqrt_price, 99999);
    }

    #[tokio::test]
    async fn test_redis_cache_reconnect() {
        let redis_url = match env::var("REDIS_URL") {
            Ok(val) => val,
            Err(_) => return, // skip if not set
        };

        let client = redis::Client::open(redis_url).unwrap();
        let conn = client.get_multiplexed_async_connection().await.unwrap();
        let cache = RedisCache::new(client.clone(), conn);

        let new_conn = cache.reconnect().await;
        assert!(new_conn.is_ok());
    }
}
