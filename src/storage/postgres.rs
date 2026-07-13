use crate::discovery::progress::{
    DiscoveryPageCommit, PoolDiscoveryFailure, PoolDiscoveryProgress,
};
use crate::models::{PoolState, PoolTickData, Token};
use crate::storage::PostgresStorageTrait;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use sqlx::{PgPool, Row};

pub struct PostgresDb {
    pool: PgPool,
}

impl PostgresDb {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Borrow the underlying connection pool (integration / e2e helpers).
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Initializes tables in PostgreSQL
    pub async fn create_tables(&self) -> Result<()> {
        // Create tokens table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS tokens (
                address VARCHAR(255) PRIMARY KEY,
                symbol VARCHAR(50) NOT NULL,
                name VARCHAR(255) NOT NULL,
                decimals SMALLINT NOT NULL
            );",
        )
        .execute(&self.pool)
        .await?;

        // Create pools table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pools (
                pool_id VARCHAR(255) PRIMARY KEY,
                dex_name VARCHAR(100) NOT NULL,
                coin_type_a VARCHAR(255) NOT NULL,
                coin_type_b VARCHAR(255) NOT NULL,
                sqrt_price VARCHAR(255) NOT NULL,
                liquidity VARCHAR(255) NOT NULL,
                fee_rate BIGINT NOT NULL,
                is_paused BOOLEAN NOT NULL
            );",
        )
        .execute(&self.pool)
        .await?;

        // Create system_config table
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS system_config (
                key VARCHAR(255) PRIMARY KEY,
                value VARCHAR(255) NOT NULL
            );",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pool_tick_data (
                pool_id VARCHAR(255) PRIMARY KEY,
                current_tick_index INT NOT NULL,
                tick_spacing INT NOT NULL
            );",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "ALTER TABLE pool_tick_data
             ADD COLUMN IF NOT EXISTS fee_growth_global_a VARCHAR(255);",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "ALTER TABLE pool_tick_data
             ADD COLUMN IF NOT EXISTS fee_growth_global_b VARCHAR(255);",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pool_ticks (
                pool_id VARCHAR(255) NOT NULL,
                tick_index INT NOT NULL,
                liquidity_net BIGINT NOT NULL,
                PRIMARY KEY (pool_id, tick_index)
            );",
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pool_discovery_progress (
                dex_name VARCHAR(100) NOT NULL,
                source_key VARCHAR(512) NOT NULL,
                discovery_source VARCHAR(50) NOT NULL DEFAULT 'event_incremental',
                last_checkpoint BIGINT,
                page_cursor TEXT,
                backfill_complete BOOLEAN NOT NULL DEFAULT FALSE,
                bootstrap_complete BOOLEAN NOT NULL DEFAULT FALSE,
                retention_limited BOOLEAN NOT NULL DEFAULT FALSE,
                generation BIGINT NOT NULL DEFAULT 0,
                pools_discovered BIGINT NOT NULL DEFAULT 0,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                PRIMARY KEY (dex_name, source_key)
            );",
        )
        .execute(&self.pool)
        .await?;

        // Migrate legacy schema (event_type column) if present.
        sqlx::query(
            "ALTER TABLE pool_discovery_progress
             ADD COLUMN IF NOT EXISTS source_key VARCHAR(512);",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "ALTER TABLE pool_discovery_progress
             ADD COLUMN IF NOT EXISTS discovery_source VARCHAR(50) NOT NULL DEFAULT 'event_incremental';",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "ALTER TABLE pool_discovery_progress
             ADD COLUMN IF NOT EXISTS bootstrap_complete BOOLEAN NOT NULL DEFAULT FALSE;",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "ALTER TABLE pool_discovery_progress
             ADD COLUMN IF NOT EXISTS retention_limited BOOLEAN NOT NULL DEFAULT FALSE;",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "ALTER TABLE pool_discovery_progress
             ADD COLUMN IF NOT EXISTS generation BIGINT NOT NULL DEFAULT 0;",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "UPDATE pool_discovery_progress
             SET source_key = event_type
             WHERE source_key IS NULL AND event_type IS NOT NULL;",
        )
        .execute(&self.pool)
        .await
        .ok();

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pool_discovery_failures (
                dex_name VARCHAR(100) NOT NULL,
                event_id VARCHAR(255) NOT NULL,
                pool_id VARCHAR(255),
                reason TEXT NOT NULL,
                attempts INT NOT NULL DEFAULT 1,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                PRIMARY KEY (dex_name, event_id)
            );",
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl PostgresStorageTrait for PostgresDb {
    async fn insert_token(&self, token: &Token) -> Result<()> {
        sqlx::query(
            "INSERT INTO tokens (address, symbol, name, decimals)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (address) 
             DO UPDATE SET symbol = EXCLUDED.symbol, name = EXCLUDED.name, decimals = EXCLUDED.decimals;",
        )
        .bind(&token.address)
        .bind(&token.symbol)
        .bind(&token.name)
        .bind(token.decimals as i16)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_token(&self, address: &str) -> Result<Option<Token>> {
        let row =
            sqlx::query("SELECT address, symbol, name, decimals FROM tokens WHERE address = $1;")
                .bind(address)
                .fetch_optional(&self.pool)
                .await?;

        if let Some(r) = row {
            let decimals: i16 = r.get("decimals");
            Ok(Some(Token {
                address: r.get("address"),
                symbol: r.get("symbol"),
                name: r.get("name"),
                decimals: decimals as u8,
            }))
        } else {
            Ok(None)
        }
    }

    async fn list_tokens(&self) -> Result<Vec<Token>> {
        let rows =
            sqlx::query("SELECT address, symbol, name, decimals FROM tokens ORDER BY symbol;")
                .fetch_all(&self.pool)
                .await?;

        let mut tokens = Vec::with_capacity(rows.len());
        for r in rows {
            let decimals: i16 = r.get("decimals");
            tokens.push(Token {
                address: r.get("address"),
                symbol: r.get("symbol"),
                name: r.get("name"),
                decimals: decimals as u8,
            });
        }
        Ok(tokens)
    }

    async fn insert_pool(&self, pool: &PoolState) -> Result<()> {
        sqlx::query(
            "INSERT INTO pools (pool_id, dex_name, coin_type_a, coin_type_b, sqrt_price, liquidity, fee_rate, is_paused)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (pool_id) 
             DO UPDATE SET 
                dex_name = EXCLUDED.dex_name,
                coin_type_a = EXCLUDED.coin_type_a,
                coin_type_b = EXCLUDED.coin_type_b,
                sqrt_price = EXCLUDED.sqrt_price,
                liquidity = EXCLUDED.liquidity,
                fee_rate = EXCLUDED.fee_rate,
                is_paused = EXCLUDED.is_paused;",
        )
        .bind(&pool.pool_id)
        .bind(&pool.dex_name)
        .bind(&pool.coin_type_a)
        .bind(&pool.coin_type_b)
        .bind(pool.sqrt_price.to_string())
        .bind(pool.liquidity.to_string())
        .bind(pool.fee_rate as i64)
        .bind(pool.is_paused)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_pool(&self, pool_id: &str) -> Result<Option<PoolState>> {
        let row = sqlx::query(
            "SELECT pool_id, dex_name, coin_type_a, coin_type_b, sqrt_price, liquidity, fee_rate, is_paused
             FROM pools WHERE pool_id = $1;",
        )
        .bind(pool_id)
        .fetch_optional(&self.pool)
        .await?;

        if let Some(r) = row {
            let sqrt_price_str: String = r.get("sqrt_price");
            let liquidity_str: String = r.get("liquidity");
            let fee_rate: i64 = r.get("fee_rate");

            let sqrt_price = sqrt_price_str
                .parse::<u128>()
                .map_err(|e| anyhow!("Failed to parse pool sqrt_price: {:?}", e))?;
            let liquidity = liquidity_str
                .parse::<u128>()
                .map_err(|e| anyhow!("Failed to parse pool liquidity: {:?}", e))?;
            let fee_rate_val = if fee_rate < 0 { 0 } else { fee_rate as u64 };

            Ok(Some(PoolState {
                pool_id: r.get("pool_id"),
                dex_name: r.get("dex_name"),
                coin_type_a: r.get("coin_type_a"),
                coin_type_b: r.get("coin_type_b"),
                sqrt_price,
                liquidity,
                fee_rate: fee_rate_val,
                is_paused: r.get("is_paused"),
            }))
        } else {
            Ok(None)
        }
    }

    async fn list_pools(&self) -> Result<Vec<PoolState>> {
        let rows = sqlx::query(
            "SELECT pool_id, dex_name, coin_type_a, coin_type_b, sqrt_price, liquidity, fee_rate, is_paused FROM pools;",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut pools = Vec::new();
        for r in rows {
            let sqrt_price_str: String = r.get("sqrt_price");
            let liquidity_str: String = r.get("liquidity");
            let fee_rate: i64 = r.get("fee_rate");

            let sqrt_price = match sqrt_price_str.parse::<u128>() {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        "Skipping pool {} due to corrupted sqrt_price: {:?}",
                        r.get::<String, _>("pool_id"),
                        e
                    );
                    continue;
                }
            };
            let liquidity = match liquidity_str.parse::<u128>() {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        "Skipping pool {} due to corrupted liquidity: {:?}",
                        r.get::<String, _>("pool_id"),
                        e
                    );
                    continue;
                }
            };
            let fee_rate_val = if fee_rate < 0 { 0 } else { fee_rate as u64 };

            pools.push(PoolState {
                pool_id: r.get("pool_id"),
                dex_name: r.get("dex_name"),
                coin_type_a: r.get("coin_type_a"),
                coin_type_b: r.get("coin_type_b"),
                sqrt_price,
                liquidity,
                fee_rate: fee_rate_val,
                is_paused: r.get("is_paused"),
            });
        }

        Ok(pools)
    }

    async fn set_reference_gas_price(&self, price: u64) -> Result<()> {
        sqlx::query(
            "INSERT INTO system_config (key, value)
             VALUES ('reference_gas_price', $1)
             ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value;",
        )
        .bind(price.to_string())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_reference_gas_price(&self) -> Result<Option<u64>> {
        let row = sqlx::query("SELECT value FROM system_config WHERE key = 'reference_gas_price';")
            .fetch_optional(&self.pool)
            .await?;

        if let Some(r) = row {
            let val_str: String = r.get("value");
            let price = val_str.parse::<u64>()?;
            Ok(Some(price))
        } else {
            Ok(None)
        }
    }

    async fn set_pool_tick_data(&self, data: &PoolTickData) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        let fee_a = data.fee_growth_global_a.map(|v| v.to_string());
        let fee_b = data.fee_growth_global_b.map(|v| v.to_string());

        sqlx::query(
            "INSERT INTO pool_tick_data (
                pool_id, current_tick_index, tick_spacing,
                fee_growth_global_a, fee_growth_global_b
             )
             VALUES ($1, $2, $3, $4, $5)
             ON CONFLICT (pool_id)
             DO UPDATE SET current_tick_index = EXCLUDED.current_tick_index,
                           tick_spacing = EXCLUDED.tick_spacing,
                           fee_growth_global_a = EXCLUDED.fee_growth_global_a,
                           fee_growth_global_b = EXCLUDED.fee_growth_global_b;",
        )
        .bind(&data.pool_id)
        .bind(data.current_tick_index)
        .bind(data.tick_spacing as i32)
        .bind(fee_a)
        .bind(fee_b)
        .execute(&mut *tx)
        .await?;

        sqlx::query("DELETE FROM pool_ticks WHERE pool_id = $1;")
            .bind(&data.pool_id)
            .execute(&mut *tx)
            .await?;

        for tick in &data.ticks {
            sqlx::query(
                "INSERT INTO pool_ticks (pool_id, tick_index, liquidity_net)
                 VALUES ($1, $2, $3);",
            )
            .bind(&data.pool_id)
            .bind(tick.tick_index)
            .bind(tick.liquidity_net as i64)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get_pool_tick_data(&self, pool_id: &str) -> Result<Option<PoolTickData>> {
        let row = sqlx::query(
            "SELECT pool_id, current_tick_index, tick_spacing,
                    fee_growth_global_a, fee_growth_global_b
             FROM pool_tick_data WHERE pool_id = $1;",
        )
        .bind(pool_id)
        .fetch_optional(&self.pool)
        .await?;

        let Some(r) = row else {
            return Ok(None);
        };

        let fee_growth_global_a = optional_u128_column(&r, "fee_growth_global_a")?;
        let fee_growth_global_b = optional_u128_column(&r, "fee_growth_global_b")?;

        let tick_rows = sqlx::query(
            "SELECT tick_index, liquidity_net FROM pool_ticks WHERE pool_id = $1 ORDER BY tick_index ASC;",
        )
        .bind(pool_id)
        .fetch_all(&self.pool)
        .await?;

        let mut ticks = Vec::with_capacity(tick_rows.len());
        for tr in tick_rows {
            let liquidity_net: i64 = tr.get("liquidity_net");
            ticks.push(crate::models::TickInfo {
                tick_index: tr.get("tick_index"),
                liquidity_net: liquidity_net as i128,
            });
        }

        Ok(Some(PoolTickData {
            pool_id: r.get("pool_id"),
            current_tick_index: r.get("current_tick_index"),
            tick_spacing: r.get::<i32, _>("tick_spacing") as u32,
            ticks,
            fee_growth_global_a,
            fee_growth_global_b,
        }))
    }

    async fn get_pool_discovery_progress(
        &self,
        dex_name: &str,
        source_key: &str,
    ) -> Result<Option<PoolDiscoveryProgress>> {
        let row = sqlx::query(
            "SELECT dex_name, source_key, discovery_source, last_checkpoint, page_cursor,
                    backfill_complete, bootstrap_complete, retention_limited, generation, pools_discovered
             FROM pool_discovery_progress
             WHERE dex_name = $1 AND source_key = $2;",
        )
        .bind(dex_name)
        .bind(source_key)
        .fetch_optional(&self.pool)
        .await?;

        let Some(r) = row else {
            return Ok(None);
        };

        let last_checkpoint: Option<i64> = r.get("last_checkpoint");
        let discovery_source: String = r.get("discovery_source");
        let discovery_source = match discovery_source.as_str() {
            "object_bootstrap" => crate::discovery::progress::DiscoverySource::ObjectBootstrap,
            _ => crate::discovery::progress::DiscoverySource::EventIncremental,
        };
        Ok(Some(PoolDiscoveryProgress {
            dex_name: r.get("dex_name"),
            source_key: r.get("source_key"),
            discovery_source,
            last_checkpoint: last_checkpoint.map(|v| v as u64),
            page_cursor: r.get("page_cursor"),
            backfill_complete: r.get("backfill_complete"),
            bootstrap_complete: r.get("bootstrap_complete"),
            retention_limited: r.get("retention_limited"),
            generation: r.get::<i64, _>("generation") as u64,
            pools_discovered: r.get::<i64, _>("pools_discovered") as u64,
        }))
    }

    async fn commit_discovery_page(&self, commit: &DiscoveryPageCommit) -> Result<()> {
        let progress = &commit.progress;
        let mut tx = self.pool.begin().await?;

        let last_cp = progress.last_checkpoint.map(|v| v as i64);
        sqlx::query(
            "INSERT INTO pool_discovery_progress
                (dex_name, source_key, discovery_source, last_checkpoint, page_cursor,
                 backfill_complete, bootstrap_complete, retention_limited, generation,
                 pools_discovered, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NOW())
             ON CONFLICT (dex_name, source_key)
             DO UPDATE SET
                discovery_source = EXCLUDED.discovery_source,
                last_checkpoint = EXCLUDED.last_checkpoint,
                page_cursor = EXCLUDED.page_cursor,
                backfill_complete = EXCLUDED.backfill_complete,
                bootstrap_complete = EXCLUDED.bootstrap_complete,
                retention_limited = EXCLUDED.retention_limited,
                generation = EXCLUDED.generation,
                pools_discovered = EXCLUDED.pools_discovered,
                updated_at = NOW();",
        )
        .bind(&progress.dex_name)
        .bind(&progress.source_key)
        .bind(progress.discovery_source.as_str())
        .bind(last_cp)
        .bind(&progress.page_cursor)
        .bind(progress.backfill_complete)
        .bind(progress.bootstrap_complete)
        .bind(progress.retention_limited)
        .bind(progress.generation as i64)
        .bind(progress.pools_discovered as i64)
        .execute(&mut *tx)
        .await?;

        for token in &commit.tokens {
            sqlx::query(
                "INSERT INTO tokens (address, symbol, name, decimals)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (address) DO UPDATE SET
                    symbol = EXCLUDED.symbol,
                    name = EXCLUDED.name,
                    decimals = EXCLUDED.decimals;",
            )
            .bind(&token.address)
            .bind(&token.symbol)
            .bind(&token.name)
            .bind(token.decimals as i16)
            .execute(&mut *tx)
            .await?;
        }

        for pool in &commit.pools {
            sqlx::query(
                "INSERT INTO pools
                    (pool_id, dex_name, coin_type_a, coin_type_b, sqrt_price, liquidity, fee_rate, is_paused)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 ON CONFLICT (pool_id) DO UPDATE SET
                    dex_name = EXCLUDED.dex_name,
                    coin_type_a = EXCLUDED.coin_type_a,
                    coin_type_b = EXCLUDED.coin_type_b,
                    sqrt_price = EXCLUDED.sqrt_price,
                    liquidity = EXCLUDED.liquidity,
                    fee_rate = EXCLUDED.fee_rate,
                    is_paused = EXCLUDED.is_paused;",
            )
            .bind(&pool.pool_id)
            .bind(&pool.dex_name)
            .bind(&pool.coin_type_a)
            .bind(&pool.coin_type_b)
            .bind(pool.sqrt_price.to_string())
            .bind(pool.liquidity.to_string())
            .bind(pool.fee_rate as i64)
            .bind(pool.is_paused)
            .execute(&mut *tx)
            .await?;
        }

        for failure in &commit.failures {
            sqlx::query(
                "INSERT INTO pool_discovery_failures (dex_name, event_id, pool_id, reason, attempts, updated_at)
                 VALUES ($1, $2, $3, $4, $5, NOW())
                 ON CONFLICT (dex_name, event_id)
                 DO UPDATE SET
                    pool_id = COALESCE(EXCLUDED.pool_id, pool_discovery_failures.pool_id),
                    reason = EXCLUDED.reason,
                    attempts = pool_discovery_failures.attempts + 1,
                    updated_at = NOW();",
            )
            .bind(&failure.dex_name)
            .bind(&failure.event_id)
            .bind(&failure.pool_id)
            .bind(&failure.reason)
            .bind(failure.attempts as i32)
            .execute(&mut *tx)
            .await?;
        }

        if !commit.resolved_failure_ids.is_empty() {
            sqlx::query(
                "DELETE FROM pool_discovery_failures
                 WHERE dex_name = $1 AND event_id = ANY($2);",
            )
            .bind(&progress.dex_name)
            .bind(&commit.resolved_failure_ids)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn resolve_discovery_failures(&self, dex_name: &str, event_ids: &[String]) -> Result<()> {
        if event_ids.is_empty() {
            return Ok(());
        }
        sqlx::query(
            "DELETE FROM pool_discovery_failures
             WHERE dex_name = $1 AND event_id = ANY($2);",
        )
        .bind(dex_name)
        .bind(event_ids)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn list_pool_discovery_failures(
        &self,
        dex_name: &str,
        limit: u32,
    ) -> Result<Vec<PoolDiscoveryFailure>> {
        let rows = sqlx::query(
            "SELECT dex_name, event_id, pool_id, reason, attempts
             FROM pool_discovery_failures
             WHERE dex_name = $1
             ORDER BY updated_at ASC
             LIMIT $2;",
        )
        .bind(dex_name)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            out.push(PoolDiscoveryFailure {
                dex_name: r.get("dex_name"),
                event_id: r.get("event_id"),
                pool_id: r.get("pool_id"),
                reason: r.get("reason"),
                attempts: r.get::<i32, _>("attempts") as u32,
            });
        }
        Ok(out)
    }
}

fn optional_u128_column(row: &sqlx::postgres::PgRow, col: &str) -> Result<Option<u128>> {
    let val: Option<String> = row.get(col);
    match val {
        None => Ok(None),
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => Ok(Some(s.parse::<u128>()?)),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::env;

    // Manual mock for unit tests when database connection is not available
    pub struct InMemoryPostgresStorage {
        pub tokens: std::sync::Mutex<std::collections::HashMap<String, Token>>,
        pub pools: std::sync::Mutex<std::collections::HashMap<String, PoolState>>,
        pub reference_gas_price: std::sync::Mutex<Option<u64>>,
        pub pool_tick_data: std::sync::Mutex<std::collections::HashMap<String, PoolTickData>>,
        pub discovery_progress:
            std::sync::Mutex<std::collections::HashMap<(String, String), PoolDiscoveryProgress>>,
        pub discovery_failures:
            std::sync::Mutex<std::collections::HashMap<(String, String), PoolDiscoveryFailure>>,
        pub list_pools_calls: std::sync::atomic::AtomicU32,
    }

    impl Default for InMemoryPostgresStorage {
        fn default() -> Self {
            Self::new()
        }
    }

    impl InMemoryPostgresStorage {
        pub fn new() -> Self {
            Self {
                tokens: std::sync::Mutex::new(std::collections::HashMap::new()),
                pools: std::sync::Mutex::new(std::collections::HashMap::new()),
                reference_gas_price: std::sync::Mutex::new(None),
                pool_tick_data: std::sync::Mutex::new(std::collections::HashMap::new()),
                discovery_progress: std::sync::Mutex::new(std::collections::HashMap::new()),
                discovery_failures: std::sync::Mutex::new(std::collections::HashMap::new()),
                list_pools_calls: std::sync::atomic::AtomicU32::new(0),
            }
        }
    }

    #[async_trait]
    impl PostgresStorageTrait for InMemoryPostgresStorage {
        async fn insert_token(&self, token: &Token) -> Result<()> {
            let mut tokens = self.tokens.lock().unwrap();
            tokens.insert(token.address.clone(), token.clone());
            Ok(())
        }

        async fn get_token(&self, address: &str) -> Result<Option<Token>> {
            let tokens = self.tokens.lock().unwrap();
            Ok(tokens.get(address).cloned())
        }

        async fn list_tokens(&self) -> Result<Vec<Token>> {
            let tokens = self.tokens.lock().unwrap();
            Ok(tokens.values().cloned().collect())
        }

        async fn insert_pool(&self, pool: &PoolState) -> Result<()> {
            let mut pools = self.pools.lock().unwrap();
            pools.insert(pool.pool_id.clone(), pool.clone());
            Ok(())
        }

        async fn get_pool(&self, pool_id: &str) -> Result<Option<PoolState>> {
            let pools = self.pools.lock().unwrap();
            Ok(pools.get(pool_id).cloned())
        }

        async fn list_pools(&self) -> Result<Vec<PoolState>> {
            self.list_pools_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let pools = self.pools.lock().unwrap();
            Ok(pools.values().cloned().collect())
        }

        async fn set_reference_gas_price(&self, price: u64) -> Result<()> {
            let mut rgp = self.reference_gas_price.lock().unwrap();
            *rgp = Some(price);
            Ok(())
        }

        async fn get_reference_gas_price(&self) -> Result<Option<u64>> {
            let rgp = self.reference_gas_price.lock().unwrap();
            Ok(*rgp)
        }

        async fn set_pool_tick_data(&self, data: &PoolTickData) -> Result<()> {
            let mut map = self.pool_tick_data.lock().unwrap();
            map.insert(data.pool_id.clone(), data.clone());
            Ok(())
        }

        async fn get_pool_tick_data(&self, pool_id: &str) -> Result<Option<PoolTickData>> {
            let map = self.pool_tick_data.lock().unwrap();
            Ok(map.get(pool_id).cloned())
        }

        async fn get_pool_discovery_progress(
            &self,
            dex_name: &str,
            source_key: &str,
        ) -> Result<Option<PoolDiscoveryProgress>> {
            let map = self.discovery_progress.lock().unwrap();
            Ok(map
                .get(&(dex_name.to_string(), source_key.to_string()))
                .cloned())
        }

        async fn commit_discovery_page(&self, commit: &DiscoveryPageCommit) -> Result<()> {
            let progress = &commit.progress;
            let mut prog = self.discovery_progress.lock().unwrap();
            prog.insert(
                (progress.dex_name.clone(), progress.source_key.clone()),
                progress.clone(),
            );
            drop(prog);

            let mut tokens = self.tokens.lock().unwrap();
            for token in &commit.tokens {
                tokens.insert(token.address.clone(), token.clone());
            }
            drop(tokens);

            let mut pools = self.pools.lock().unwrap();
            for pool in &commit.pools {
                pools.insert(pool.pool_id.clone(), pool.clone());
            }
            drop(pools);

            let mut fail_map = self.discovery_failures.lock().unwrap();
            for failure in &commit.failures {
                let key = (failure.dex_name.clone(), failure.event_id.clone());
                if let Some(existing) = fail_map.get_mut(&key) {
                    existing.attempts += 1;
                    existing.reason = failure.reason.clone();
                    if failure.pool_id.is_some() {
                        existing.pool_id = failure.pool_id.clone();
                    }
                } else {
                    fail_map.insert(key, failure.clone());
                }
            }
            for event_id in &commit.resolved_failure_ids {
                fail_map.remove(&(progress.dex_name.clone(), event_id.clone()));
            }
            Ok(())
        }

        async fn resolve_discovery_failures(
            &self,
            dex_name: &str,
            event_ids: &[String],
        ) -> Result<()> {
            let mut fail_map = self.discovery_failures.lock().unwrap();
            for event_id in event_ids {
                fail_map.remove(&(dex_name.to_string(), event_id.clone()));
            }
            Ok(())
        }

        async fn list_pool_discovery_failures(
            &self,
            dex_name: &str,
            limit: u32,
        ) -> Result<Vec<PoolDiscoveryFailure>> {
            let map = self.discovery_failures.lock().unwrap();
            let mut out: Vec<PoolDiscoveryFailure> = map
                .values()
                .filter(|f| f.dex_name == dex_name)
                .cloned()
                .collect();
            out.sort_by(|a, b| a.event_id.cmp(&b.event_id));
            out.truncate(limit as usize);
            Ok(out)
        }
    }

    #[tokio::test]
    async fn test_in_memory_discovery_progress_round_trip() {
        let storage = InMemoryPostgresStorage::new();
        let progress = PoolDiscoveryProgress::new_event_incremental(
            "Cetus",
            "0xpkg::factory::CreatePoolEvent",
        );
        let mut progress = progress;
        progress.last_checkpoint = Some(100);
        progress.page_cursor = Some("cursor_1".to_string());
        progress.pools_discovered = 3;
        let failure = PoolDiscoveryFailure {
            dex_name: "Cetus".to_string(),
            event_id: "evt_1".to_string(),
            pool_id: Some("0xpool".to_string()),
            reason: "fetch failed".to_string(),
            attempts: 1,
        };
        storage
            .commit_discovery_page(&DiscoveryPageCommit {
                progress: progress.clone(),
                failures: vec![failure],
                ..Default::default()
            })
            .await
            .unwrap();
        let fetched = storage
            .get_pool_discovery_progress("Cetus", "0xpkg::factory::CreatePoolEvent")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.page_cursor.as_deref(), Some("cursor_1"));
        let failures = storage
            .list_pool_discovery_failures("Cetus", 10)
            .await
            .unwrap();
        assert_eq!(failures.len(), 1);
    }

    #[tokio::test]
    async fn test_in_memory_pool_tick_data() {
        let storage = InMemoryPostgresStorage::new();
        let data = PoolTickData {
            pool_id: "0x_tick_pool".to_string(),
            current_tick_index: 50,
            tick_spacing: 60,
            ticks: vec![crate::models::TickInfo {
                tick_index: 60,
                liquidity_net: -1000,
            }],
            fee_growth_global_a: Some(12345),
            fee_growth_global_b: None,
        };
        storage.set_pool_tick_data(&data).await.unwrap();
        let fetched = storage
            .get_pool_tick_data("0x_tick_pool")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched, data);
    }

    #[tokio::test]
    async fn test_in_memory_storage() {
        let storage = InMemoryPostgresStorage::new();
        let token = Token {
            address: "0x123".to_string(),
            symbol: "TEST".to_string(),
            name: "Test Token".to_string(),
            decimals: 9,
        };

        storage.insert_token(&token).await.unwrap();
        let fetched = storage.get_token("0x123").await.unwrap().unwrap();
        assert_eq!(fetched.symbol, "TEST");
    }

    #[tokio::test]
    async fn test_live_postgres_if_available() {
        // Runs only if DATABASE_URL is set in environment (e.g. during manual integration testing)
        let db_url = match env::var("DATABASE_URL") {
            Ok(val) => val,
            Err(_) => return, // skip if not set
        };

        let pool = PgPool::connect(&db_url).await.unwrap();
        let db = PostgresDb::new(pool);
        db.create_tables().await.unwrap();

        let token = Token {
            address: "0x_pg_test_token".to_string(),
            symbol: "PGT".to_string(),
            name: "Postgres Test Token".to_string(),
            decimals: 9,
        };

        db.insert_token(&token).await.unwrap();
        let fetched = db.get_token("0x_pg_test_token").await.unwrap().unwrap();
        assert_eq!(fetched.symbol, "PGT");
    }
}
