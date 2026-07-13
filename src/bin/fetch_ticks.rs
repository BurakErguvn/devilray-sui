use devilray_sui::collectors::collector_for_dex_name;
use devilray_sui::storage::postgres::PostgresDb;
use devilray_sui::storage::redis::RedisCache;
use devilray_sui::storage::{PostgresStorageTrait, RedisCacheTrait};
use devilray_sui::sui_client::SuiClient;
use std::env;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "fetch_ticks=info".into()),
        )
        .init();

    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() < 2 {
        eprintln!("Usage: cargo run --bin fetch_ticks -- <dex> <pool_id>");
        eprintln!("  dex: cetus | turbos | magma | momentum");
        std::process::exit(1);
    }

    let dex = args.remove(0);
    let pool_id = args.remove(0);

    let rpc_url =
        env::var("RPC_URL").unwrap_or_else(|_| "https://fullnode.mainnet.sui.io:443".to_string());
    let sui_client = Arc::new(SuiClient::new(rpc_url, String::new()));

    let collector = match collector_for_dex_name(&dex) {
        Some(c) => c,
        None => {
            eprintln!(
                "Unsupported dex '{}'. Use cetus, turbos, magma, or momentum.",
                dex
            );
            std::process::exit(1);
        }
    };

    let tick_data = collector
        .fetch_tick_data(sui_client.as_ref(), &pool_id)
        .await?;

    println!("DEX: {}", dex);
    println!("Pool: {}", tick_data.pool_id);
    println!("Current tick: {}", tick_data.current_tick_index);
    println!("Tick spacing: {}", tick_data.tick_spacing);
    println!("Initialized ticks in window: {}", tick_data.ticks.len());
    for t in &tick_data.ticks {
        println!("  tick={} liquidity_net={}", t.tick_index, t.liquidity_net);
    }

    if let Ok(pg_url) = env::var("DATABASE_URL") {
        let pool = sqlx::PgPool::connect(&pg_url).await?;
        let pg_db = PostgresDb::new(pool);
        pg_db.create_tables().await?;
        pg_db.set_pool_tick_data(&tick_data).await?;
        println!("Persisted to PostgreSQL.");
    }

    if let Ok(redis_url) = env::var("REDIS_URL") {
        let client = redis::Client::open(redis_url)?;
        let conn = client.get_multiplexed_async_connection().await?;
        let cache = RedisCache::new(client, conn);
        cache.set_pool_tick_data(&tick_data).await?;
        println!("Cached in Redis (TTL 300s).");
    }

    Ok(())
}
