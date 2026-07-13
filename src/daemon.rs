use crate::models::Token;
use crate::queue::RedisMessageQueue;
use crate::storage::clickhouse_storage::ClickhouseClient;
use crate::storage::postgres::PostgresDb;
use crate::storage::redis::RedisCache;
use crate::storage::{ClickhouseAnalyticsTrait, PostgresStorageTrait, RedisCacheTrait, SwapEvent};
use crate::sui_client::SuiClient;
use crate::workers::{
    DynamicWorkerConfig, StaticWorkerConfig, db_writer::DatabaseWriter,
    dynamic_worker::DynamicPoolManager, static_worker::StaticPoolManager,
};
use anyhow::{Result, anyhow};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};

pub const DEFAULT_QUEUE_NAME: &str = "devilray_write_queue";

/// Runtime configuration loaded from environment variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonConfig {
    pub pg_url: String,
    pub redis_url: String,
    pub clickhouse_url: Option<String>,
    pub rpc_url: String,
    pub graphql_url: String,
    pub ws_url: String,
    pub bind_addr: String,
}

impl DaemonConfig {
    pub fn from_env() -> Result<Self> {
        Self::from_env_map(|k| std::env::var(k).ok())
    }

    pub fn from_env_map<F>(get: F) -> Result<Self>
    where
        F: Fn(&str) -> Option<String>,
    {
        let pg_url = get("DATABASE_URL").ok_or_else(|| anyhow!("DATABASE_URL required"))?;
        let redis_url = get("REDIS_URL").ok_or_else(|| anyhow!("REDIS_URL required"))?;
        Ok(Self {
            pg_url,
            redis_url,
            clickhouse_url: get("CLICKHOUSE_URL").filter(|s| !s.is_empty()),
            rpc_url: get("RPC_URL")
                .unwrap_or_else(|| "https://fullnode.mainnet.sui.io:443".to_string()),
            graphql_url: get("GRAPHQL_URL")
                .unwrap_or_else(|| "https://graphql.mainnet.sui.io/graphql".to_string()),
            ws_url: get("WEBSOCKET_URL")
                .unwrap_or_else(|| "wss://fullnode.mainnet.sui.io:443".to_string()),
            bind_addr: get("BIND_ADDR").unwrap_or_else(|| "0.0.0.0:3000".to_string()),
        })
    }
}

/// Retry an async connect operation with fixed backoff between attempts.
pub async fn connect_with_retry<F, Fut, T, E>(
    mut connect: F,
    max_attempts: u32,
    backoff_ms: u64,
) -> std::result::Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, E>>,
{
    let mut last_err = None;
    for attempt in 1..=max_attempts {
        match connect().await {
            Ok(value) => return Ok(value),
            Err(err) => {
                tracing::warn!(
                    attempt,
                    max_attempts,
                    "connect attempt failed, retrying in {}ms",
                    backoff_ms
                );
                last_err = Some(err);
                if attempt < max_attempts {
                    tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
                }
            }
        }
    }
    Err(last_err.expect("max_attempts must be >= 1"))
}

struct MockClickhouse {
    logs: std::sync::Mutex<Vec<SwapEvent>>,
}

#[async_trait::async_trait]
impl ClickhouseAnalyticsTrait for MockClickhouse {
    async fn insert_swap_event(&self, event: &SwapEvent) -> Result<()> {
        let mut logs = self.logs.lock().unwrap();
        logs.push(event.clone());
        tracing::debug!(
            event_id = %event.event_id,
            pool_id = %event.pool_id,
            "mock clickhouse buffered swap event"
        );
        Ok(())
    }

    async fn get_swap_events(&self, limit: u64) -> Result<Vec<SwapEvent>> {
        let logs = self.logs.lock().unwrap();
        Ok(logs.iter().take(limit as usize).cloned().collect())
    }
}

pub async fn seed_known_tokens(pg_db: &dyn PostgresStorageTrait) -> Result<()> {
    let sui_token = Token {
        address: "0x2::sui::SUI".to_string(),
        symbol: "SUI".to_string(),
        name: "Sui".to_string(),
        decimals: 9,
    };
    let usdc_token = Token {
        address: "0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN"
            .to_string(),
        symbol: "USDC".to_string(),
        name: "USDC".to_string(),
        decimals: 6,
    };
    pg_db.insert_token(&sui_token).await?;
    pg_db.insert_token(&usdc_token).await?;
    Ok(())
}

/// Run the full worker daemon until `shutdown_rx` signals shutdown.
pub async fn run_daemon(cfg: DaemonConfig, mut shutdown_rx: watch::Receiver<bool>) -> Result<()> {
    tracing::info!("connecting to PostgreSQL");
    let pg_url = cfg.pg_url.clone();
    let pg_pool = connect_with_retry(|| sqlx::PgPool::connect(&pg_url), 5, 2_000).await?;
    let pg_db = Arc::new(PostgresDb::new(pg_pool));
    pg_db.create_tables().await?;

    tracing::info!("connecting to Redis");
    let redis_url = cfg.redis_url.clone();
    let redis_client = connect_with_retry(
        || async { redis::Client::open(redis_url.clone()) },
        5,
        2_000,
    )
    .await?;
    let redis_conn =
        connect_with_retry(|| redis_client.get_multiplexed_async_connection(), 5, 2_000).await?;
    let redis_cache = Arc::new(RedisCache::new(redis_client.clone(), redis_conn.clone()));

    let queue = Arc::new(RedisMessageQueue::new(redis_conn));
    let queue_name = DEFAULT_QUEUE_NAME.to_string();

    let clickhouse_db: Arc<dyn ClickhouseAnalyticsTrait> =
        if let Some(ch_url) = cfg.clickhouse_url.clone() {
            tracing::info!("connecting to ClickHouse");
            let ch_url_clone = ch_url.clone();
            let client = connect_with_retry(
                || {
                    let ch_url_clone = ch_url_clone.clone();
                    async move {
                        let ch = ClickhouseClient::new(
                            clickhouse::Client::default().with_url(&ch_url_clone),
                        );
                        ch.create_tables().await?;
                        Ok::<ClickhouseClient, anyhow::Error>(ch)
                    }
                },
                5,
                2_000,
            )
            .await?;
            Arc::new(client)
        } else {
            tracing::info!("CLICKHOUSE_URL not set; using mock ClickHouse");
            Arc::new(MockClickhouse {
                logs: std::sync::Mutex::new(Vec::new()),
            })
        };

    let sui_client = Arc::new(SuiClient::new(cfg.rpc_url.clone(), cfg.graphql_url.clone()));
    let (broadcast_tx, _) = broadcast::channel(1024);

    seed_known_tokens(pg_db.as_ref()).await?;

    tracing::info!("launching static pool discovery worker");
    let static_config = static_worker_config_from_env();
    let topology_ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let static_manager = Arc::new(StaticPoolManager::new(
        pg_db.clone(),
        redis_cache.clone(),
        sui_client.clone(),
        queue.clone(),
        queue_name.clone(),
        static_config,
        topology_ready.clone(),
    ));
    let static_shutdown_rx = shutdown_rx.clone();
    let static_manager_clone = static_manager.clone();
    let static_handle = tokio::spawn(async move {
        let mut rx = static_shutdown_rx;
        if let Err(e) = static_manager_clone.run(&mut rx).await {
            tracing::error!(error = ?e, "static pool manager exited with error");
        }
    });

    // Wait for initial object-bootstrap topology (bounded wait; degraded if pools already exist).
    let existing = pg_db.list_pools().await?;
    if existing.is_empty() {
        tracing::info!("waiting for initial pool discovery bootstrap");
        for _ in 0..600 {
            if topology_ready.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            if *shutdown_rx.borrow() {
                break;
            }
        }
    } else {
        topology_ready.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    let pools = pg_db.list_pools().await?;
    tracing::info!(pool_count = pools.len(), "pools registered in PostgreSQL");

    tracing::info!("launching dynamic worker pool");
    let dynamic_manager = Arc::new(DynamicPoolManager::new(
        pg_db.clone(),
        redis_cache.clone(),
        sui_client.clone(),
        queue.clone(),
        queue_name.clone(),
        broadcast_tx.clone(),
        DynamicWorkerConfig {
            num_workers: 2,
            poll_interval_ms: 3000,
            max_retries: 3,
            retry_backoff_ms: 300,
            websocket_url: cfg.ws_url.clone(),
            ws_reconnect_interval_ms: 3000,
        },
    ));

    let manager_shutdown_rx = shutdown_rx.clone();
    let manager_clone = dynamic_manager.clone();
    let manager_handle = tokio::spawn(async move {
        let mut rx = manager_shutdown_rx;
        if let Err(e) = manager_clone.run(&mut rx).await {
            tracing::error!(error = ?e, "dynamic pool manager exited with error");
        }
    });

    tracing::info!("launching database writer");
    let db_writer = Arc::new(DatabaseWriter::new(
        pg_db.clone(),
        clickhouse_db.clone(),
        queue.clone(),
        queue_name.clone(),
    ));
    let db_writer_shutdown_rx = shutdown_rx.clone();
    let db_writer_handle = tokio::spawn(async move {
        let mut rx = db_writer_shutdown_rx;
        if let Err(e) = db_writer.run(&mut rx).await {
            tracing::error!(error = ?e, "database writer exited with error");
        }
    });

    tracing::info!(bind_addr = %cfg.bind_addr, "launching REST and WebSocket server");
    let app_state = crate::api::websocket::ServerAppState::new(
        broadcast_tx.clone(),
        pg_db.clone() as Arc<dyn PostgresStorageTrait>,
        redis_cache.clone() as Arc<dyn RedisCacheTrait>,
        topology_ready.clone(),
        sui_client.clone() as Arc<dyn crate::sui_client::SuiClientTrait>,
    );
    let app = axum::Router::new()
        .route(
            "/health",
            axum::routing::get(crate::api::info::handle_health),
        )
        .route(
            "/readyz",
            axum::routing::get(crate::api::info::handle_readyz),
        )
        .route(
            "/api/v1/tokens",
            axum::routing::get(crate::api::info::handle_list_tokens),
        )
        .route(
            "/metrics",
            axum::routing::get(crate::api::metrics::handle_metrics),
        )
        .route("/ws", axum::routing::get(crate::api::websocket::ws_handler))
        .route(
            "/api/quote",
            axum::routing::get(crate::api::quote::handle_quote),
        )
        .route(
            "/api/build_tx",
            axum::routing::post(crate::api::quote::handle_build_tx),
        )
        .with_state(app_state);

    let bind_addr = cfg.bind_addr.clone();
    let ws_server_shutdown_rx = shutdown_rx.clone();
    let ws_server_handle = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(&bind_addr)
            .await
            .unwrap_or_else(|e| panic!("failed to bind {bind_addr}: {e}"));
        tracing::info!(bind_addr = %bind_addr, "server listening");
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let mut rx = ws_server_shutdown_rx;
                let _ = rx.changed().await;
            })
            .await
            .expect("axum server failed");
    });

    tracing::info!("daemon running; press Ctrl+C to stop");
    let _ = shutdown_rx.changed().await;

    tracing::info!("shutdown signal received, draining workers");
    static_handle.await?;
    manager_handle.await?;
    db_writer_handle.await?;
    ws_server_handle.await?;

    Ok(())
}

fn env_parse_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn env_parse_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn static_worker_config_from_env() -> StaticWorkerConfig {
    StaticWorkerConfig {
        num_workers: 1,
        scan_interval_secs: env_parse_u64("STATIC_SCAN_INTERVAL_SECS", 3600),
        discovery_page_size: env_parse_u32("DISCOVERY_PAGE_SIZE", 50),
        discovery_max_pages_per_scan: env_parse_u32("DISCOVERY_MAX_PAGES", 100),
        discovery_inter_page_ms: env_parse_u64("DISCOVERY_INTER_PAGE_MS", 100),
        reconciliation_every_n_scans: env_parse_u32("RECONCILIATION_EVERY_N_SCANS", 1),
        discovery_hydrate_concurrency: env_parse_u32("DISCOVERY_HYDRATE_CONCURRENCY", 8) as usize,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn env_map(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let map: std::collections::HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    #[test]
    fn test_daemon_config_requires_database_url() {
        let err =
            DaemonConfig::from_env_map(env_map(&[("REDIS_URL", "redis://localhost")])).unwrap_err();
        assert!(err.to_string().contains("DATABASE_URL"));
    }

    #[test]
    fn test_daemon_config_requires_redis_url() {
        let err =
            DaemonConfig::from_env_map(env_map(&[("DATABASE_URL", "postgres://localhost/db")]))
                .unwrap_err();
        assert!(err.to_string().contains("REDIS_URL"));
    }

    #[test]
    fn test_daemon_config_defaults_and_optional_clickhouse() {
        let cfg = DaemonConfig::from_env_map(env_map(&[
            ("DATABASE_URL", "postgres://localhost/db"),
            ("REDIS_URL", "redis://localhost"),
            ("CLICKHOUSE_URL", ""),
            ("BIND_ADDR", "127.0.0.1:4000"),
        ]))
        .unwrap();

        assert_eq!(cfg.pg_url, "postgres://localhost/db");
        assert_eq!(cfg.redis_url, "redis://localhost");
        assert_eq!(cfg.clickhouse_url, None);
        assert_eq!(cfg.bind_addr, "127.0.0.1:4000");
        assert_eq!(cfg.rpc_url, "https://fullnode.mainnet.sui.io:443");
    }

    #[tokio::test]
    async fn test_connect_with_retry_succeeds_after_failures() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = connect_with_retry(
            || {
                let attempts = attempts_clone.clone();
                async move {
                    let n = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                    if n < 3 { Err("temporary") } else { Ok(42u32) }
                }
            },
            5,
            1,
        )
        .await;

        assert_eq!(result, Ok(42));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_connect_with_retry_fails_after_max_attempts() {
        let attempts = Arc::new(AtomicU32::new(0));
        let attempts_clone = attempts.clone();

        let result = connect_with_retry(
            || {
                let attempts = attempts_clone.clone();
                async move {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    Err::<u32, &str>("always fails")
                }
            },
            3,
            1,
        )
        .await;

        assert_eq!(result, Err("always fails"));
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }
}
