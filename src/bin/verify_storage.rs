use devilray_sui::models::{PoolState, Token};
use devilray_sui::storage::{
    ClickhouseAnalyticsTrait, PostgresStorageTrait, RedisCacheTrait, SwapEvent,
    clickhouse_storage::ClickhouseClient, postgres::PostgresDb, redis::RedisCache,
};
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("==================================================");
    println!("DevilRay - Storage Layer Verification Tool");
    println!("==================================================");

    // 1. PostgreSQL (Cold Path) Verification
    let pg_url = env::var("DATABASE_URL").unwrap_or_else(|_| {
        println!("DATABASE_URL not set, skipping PostgreSQL verification.");
        "".to_string()
    });

    if !pg_url.is_empty() {
        println!("\n[1/3] Verifying PostgreSQL Connection...");
        let pool = sqlx::PgPool::connect(&pg_url).await?;
        let pg_db = PostgresDb::new(pool);

        println!("Creating PostgreSQL tables...");
        pg_db.create_tables().await?;

        let sample_token = Token {
            address: "0x2::sui::SUI".to_string(),
            symbol: "SUI".to_string(),
            name: "Sui".to_string(),
            decimals: 9,
        };

        println!("Inserting sample token...");
        pg_db.insert_token(&sample_token).await?;

        println!("Retrieving sample token...");
        let fetched_token = pg_db.get_token("0x2::sui::SUI").await?;
        match fetched_token {
            Some(token) => {
                println!("SUCCESS! Fetched token: {} ({})", token.name, token.symbol);
            }
            None => {
                println!("FAILED: Token not found in database.");
            }
        }

        let sample_pool = PoolState {
            pool_id: "0xcf994611fd4c48e277ce3ffd4d4364c914af2c3cbb05f7bf6facd371de688630"
                .to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a:
                "0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN"
                    .to_string(),
            coin_type_b: "0x2::sui::SUI".to_string(),
            sqrt_price: 692304700626636288339,
            liquidity: 2977078914442,
            fee_rate: 2500,
            is_paused: false,
        };

        println!("Inserting sample pool...");
        pg_db.insert_pool(&sample_pool).await?;

        println!("Retrieving sample pool...");
        let fetched_pool = pg_db
            .get_pool("0xcf994611fd4c48e277ce3ffd4d4364c914af2c3cbb05f7bf6facd371de688630")
            .await?;
        match fetched_pool {
            Some(pool) => {
                println!(
                    "SUCCESS! Fetched pool on {}: liquidity={}",
                    pool.dex_name, pool.liquidity
                );
            }
            None => {
                println!("FAILED: Pool not found in database.");
            }
        }
    }

    // 2. Redis (Hot Path) Verification
    let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| {
        println!("REDIS_URL not set, skipping Redis verification.");
        "".to_string()
    });

    if !redis_url.is_empty() {
        println!("\n[2/3] Verifying Redis Connection...");
        let client = redis::Client::open(redis_url)?;
        let conn = client.get_multiplexed_async_connection().await?;
        let cache = RedisCache::new(client.clone(), conn);

        let cache_pool = PoolState {
            pool_id: "redis_hot_pool_test".to_string(),
            dex_name: "Turbos".to_string(),
            coin_type_a: "0xcoinA".to_string(),
            coin_type_b: "0xcoinB".to_string(),
            sqrt_price: 123456789012345,
            liquidity: 987654321,
            fee_rate: 3000,
            is_paused: false,
        };

        println!("Setting pool state in Redis...");
        cache.set_pool_state(&cache_pool).await?;

        println!("Retrieving pool state from Redis...");
        let fetched = cache.get_pool_state("redis_hot_pool_test").await?;
        match fetched {
            Some(pool) => {
                println!(
                    "SUCCESS! Cached pool found. Sqrt Price: {}",
                    pool.sqrt_price
                );
            }
            None => {
                println!("FAILED: Cached pool state not found in Redis.");
            }
        }
    }

    // 3. ClickHouse (Analytics) Verification
    let ch_url = env::var("CLICKHOUSE_URL").unwrap_or_else(|_| {
        println!("CLICKHOUSE_URL not set, skipping ClickHouse verification.");
        "".to_string()
    });

    if !ch_url.is_empty() {
        println!("\n[3/3] Verifying ClickHouse Connection...");
        let client = clickhouse::Client::default().with_url(&ch_url);
        let ch = ClickhouseClient::new(client);

        println!("Creating ClickHouse tables...");
        ch.create_tables().await?;

        let now_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let event = SwapEvent {
            event_id: format!("tx_log_{}", now_epoch),
            timestamp: now_epoch,
            pool_id: "0xcf994611fd4c48e277ce3ffd4d4364c914af2c3cbb05f7bf6facd371de688630"
                .to_string(),
            dex_name: "Cetus".to_string(),
            sender: "0xsender_verify".to_string(),
            amount_in: "10000000000".to_string(),
            amount_out: "9950000000".to_string(),
            coin_in: "0x2::sui::SUI".to_string(),
            coin_out:
                "0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN"
                    .to_string(),
        };

        println!("Logging swap event to ClickHouse...");
        ch.insert_swap_event(&event).await?;

        println!("Retrieving swap events from ClickHouse...");
        let logs = ch.get_swap_events(5).await?;
        if logs.iter().any(|e| e.event_id == event.event_id) {
            println!(
                "SUCCESS! Swap event correctly logged and read back. Logs found: {}",
                logs.len()
            );
        } else {
            println!("FAILED: Logged event not found in retrieved log list.");
        }
    }

    println!("\n==================================================");
    println!("Verification process completed.");
    println!("==================================================");

    Ok(())
}
