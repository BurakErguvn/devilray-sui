use crate::models::{PoolState, PoolTickData};
use crate::sui_client::SuiClientTrait;
use anyhow::{Result, anyhow};
use async_trait::async_trait;

pub mod cetus;
pub mod fee_growth_fixtures;
pub mod magma;
pub mod momentum;
pub mod tick_fetch;
pub mod turbos;

/// Max tick indices (each side of current) to fetch from chain per pool.
pub const MAX_TICK_FETCH_WINDOW: i32 = 200;

#[async_trait]
pub trait DexDataCollector: Send + Sync {
    /// Returns the name of the DEX (e.g. "Cetus", "Turbos", etc.)
    fn dex_name(&self) -> &'static str;

    /// Fetches and parses a specific pool's state from the Sui network
    async fn fetch_pool(&self, client: &dyn SuiClientTrait, pool_id: &str) -> Result<PoolState>;

    /// Fetches tick liquidity data for tick-aware routing (optional per DEX).
    async fn fetch_tick_data(
        &self,
        _client: &dyn SuiClientTrait,
        _pool_id: &str,
    ) -> Result<PoolTickData> {
        Err(anyhow!("tick data not supported for {}", self.dex_name()))
    }
}

/// Returns a collector instance for a registered pool `dex_name`, if tick fetch is supported.
pub fn collector_for_dex_name(dex_name: &str) -> Option<Box<dyn DexDataCollector>> {
    let lower = dex_name.to_lowercase();
    if lower.contains("cetus") {
        Some(Box::new(cetus::CetusCollector::new()))
    } else if lower.contains("turbos") {
        Some(Box::new(turbos::TurbosCollector::new()))
    } else if lower.contains("magma") {
        Some(Box::new(magma::MagmaCollector::new()))
    } else if lower.contains("momentum") {
        Some(Box::new(momentum::MomentumCollector::new()))
    } else {
        None
    }
}
