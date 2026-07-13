use crate::discovery::progress::{
    DiscoveryPageCommit, PoolDiscoveryFailure, PoolDiscoveryProgress,
};
use crate::models::{PoolState, PoolTickData, Token};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub mod clickhouse_storage;
pub mod postgres;
pub mod redis;

#[async_trait]
pub trait PostgresStorageTrait: Send + Sync {
    /// Inserts or updates a token in the cold storage
    async fn insert_token(&self, token: &Token) -> Result<()>;

    /// Fetches a token by its address
    async fn get_token(&self, address: &str) -> Result<Option<Token>>;

    /// Lists all registered tokens (cold path)
    async fn list_tokens(&self) -> Result<Vec<Token>>;

    /// Inserts or updates a pool metadata
    async fn insert_pool(&self, pool: &PoolState) -> Result<()>;

    /// Fetches a pool metadata by its ID
    async fn get_pool(&self, pool_id: &str) -> Result<Option<PoolState>>;

    /// Lists all registered pools
    async fn list_pools(&self) -> Result<Vec<PoolState>>;

    /// Saves the current reference gas price
    async fn set_reference_gas_price(&self, price: u64) -> Result<()>;

    /// Fetches the saved reference gas price
    async fn get_reference_gas_price(&self) -> Result<Option<u64>>;

    /// Persists tick liquidity data for a pool (cold path)
    async fn set_pool_tick_data(&self, data: &PoolTickData) -> Result<()>;

    /// Loads tick liquidity data for a pool
    async fn get_pool_tick_data(&self, pool_id: &str) -> Result<Option<PoolTickData>>;

    /// Reads paginated discovery progress for a DEX event type.
    async fn get_pool_discovery_progress(
        &self,
        dex_name: &str,
        event_type: &str,
    ) -> Result<Option<PoolDiscoveryProgress>>;

    /// Atomically commits a scanned page: pools, tokens, progress, failures (all-or-nothing).
    async fn commit_discovery_page(&self, commit: &DiscoveryPageCommit) -> Result<()>;

    /// Removes resolved discovery failures after a successful retry.
    async fn resolve_discovery_failures(&self, dex_name: &str, event_ids: &[String]) -> Result<()>;

    /// Lists retryable discovery failures for a DEX.
    async fn list_pool_discovery_failures(
        &self,
        dex_name: &str,
        limit: u32,
    ) -> Result<Vec<PoolDiscoveryFailure>>;
}

#[async_trait]
pub trait RedisCacheTrait: Send + Sync {
    /// Caches active pool states in memory (Hot Path)
    async fn set_pool_state(&self, state: &PoolState) -> Result<()>;

    /// Retrieves active pool states from memory
    async fn get_pool_state(&self, pool_id: &str) -> Result<Option<PoolState>>;

    /// Caches the current reference gas price
    async fn set_reference_gas_price(&self, price: u64) -> Result<()>;

    /// Retrieves the reference gas price from cache
    async fn get_reference_gas_price(&self) -> Result<Option<u64>>;

    /// Caches tick liquidity data (hot path)
    async fn set_pool_tick_data(&self, data: &PoolTickData) -> Result<()>;

    /// Retrieves tick liquidity data from cache
    async fn get_pool_tick_data(&self, pool_id: &str) -> Result<Option<PoolTickData>>;

    /// Caches the active (non-paused) pool topology snapshot
    async fn set_active_pools(&self, pools: &[PoolState]) -> Result<()>;

    /// Retrieves the active pool topology snapshot
    async fn get_active_pools(&self) -> Result<Option<Vec<PoolState>>>;

    /// Caches all token metadata
    async fn set_all_tokens(&self, tokens: &[Token]) -> Result<()>;

    /// Retrieves all cached token metadata
    async fn get_all_tokens(&self) -> Result<Option<Vec<Token>>>;
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, clickhouse::Row)]
pub struct SwapEvent {
    pub event_id: String,
    pub timestamp: u64, // epoch seconds
    pub pool_id: String,
    pub dex_name: String,
    pub sender: String,
    pub amount_in: String,
    pub amount_out: String,
    pub coin_in: String,
    pub coin_out: String,
}

#[async_trait]
pub trait ClickhouseAnalyticsTrait: Send + Sync {
    /// Logs a swap transaction in the analytics engine
    async fn insert_swap_event(&self, event: &SwapEvent) -> Result<()>;

    /// Retrieves recent swap logs
    async fn get_swap_events(&self, limit: u64) -> Result<Vec<SwapEvent>>;
}
