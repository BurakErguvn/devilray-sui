#![cfg(feature = "integration")]

use devilray_sui::models::{PoolState, PoolTickData, TickInfo, Token};
use devilray_sui::storage::PostgresStorageTrait;
use devilray_sui::storage::postgres::PostgresDb;
use sqlx::PgPool;
use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

async fn connect_pg() -> Option<PostgresDb> {
    let db_url = env::var("DATABASE_URL").ok()?;
    let pool = PgPool::connect(&db_url).await.ok()?;
    let db = PostgresDb::new(pool);
    db.create_tables().await.ok()?;
    Some(db)
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

#[tokio::test]
async fn pg_token_round_trip() {
    let Some(pg) = connect_pg().await else {
        return;
    };
    let suffix = unique_suffix();
    let token = Token {
        address: format!("0x_token_{suffix}"),
        symbol: "TST".to_string(),
        name: "Test Token".to_string(),
        decimals: 9,
    };

    pg.insert_token(&token).await.unwrap();
    let fetched = pg.get_token(&token.address).await.unwrap().unwrap();
    assert_eq!(fetched, token);
}

#[tokio::test]
async fn pg_pool_round_trip() {
    let Some(pg) = connect_pg().await else {
        return;
    };
    let suffix = unique_suffix();
    let pool = PoolState {
        pool_id: format!("0x_pool_{suffix}"),
        dex_name: "Cetus".to_string(),
        coin_type_a: "A".to_string(),
        coin_type_b: "B".to_string(),
        sqrt_price: 123_456,
        liquidity: 789,
        fee_rate: 2500,
        is_paused: false,
    };

    pg.insert_pool(&pool).await.unwrap();
    let fetched = pg.get_pool(&pool.pool_id).await.unwrap().unwrap();
    assert_eq!(fetched.sqrt_price, pool.sqrt_price);
    assert_eq!(fetched.liquidity, pool.liquidity);
}

#[tokio::test]
async fn pg_tick_data_round_trip() {
    let Some(pg) = connect_pg().await else {
        return;
    };
    let suffix = unique_suffix();
    let pool_id = format!("0x_tick_pool_{suffix}");
    let tick_data = PoolTickData {
        pool_id: pool_id.clone(),
        current_tick_index: 42,
        tick_spacing: 60,
        ticks: vec![TickInfo {
            tick_index: 60,
            liquidity_net: 1000,
        }],
        ..Default::default()
    };

    pg.set_pool_tick_data(&tick_data).await.unwrap();
    let fetched = pg.get_pool_tick_data(&pool_id).await.unwrap().unwrap();
    assert_eq!(fetched.current_tick_index, 42);
    assert_eq!(fetched.ticks.len(), 1);
}

#[tokio::test]
async fn pg_list_tokens_and_pools() {
    let Some(pg) = connect_pg().await else {
        return;
    };
    let suffix = unique_suffix();

    for i in 0..2 {
        pg.insert_token(&Token {
            address: format!("0x_list_token_{suffix}_{i}"),
            symbol: format!("T{i}"),
            name: format!("Token {i}"),
            decimals: 6,
        })
        .await
        .unwrap();
        pg.insert_pool(&PoolState {
            pool_id: format!("0x_list_pool_{suffix}_{i}"),
            dex_name: "Cetus".to_string(),
            coin_type_a: "A".to_string(),
            coin_type_b: "B".to_string(),
            sqrt_price: 100 + i as u128,
            liquidity: 200,
            fee_rate: 3000,
            is_paused: false,
        })
        .await
        .unwrap();
    }

    let tokens = pg.list_tokens().await.unwrap();
    let pools = pg.list_pools().await.unwrap();
    assert!(tokens.len() >= 2);
    assert!(pools.len() >= 2);
}

#[tokio::test]
async fn pg_reference_gas_price() {
    let Some(pg) = connect_pg().await else {
        return;
    };
    let price = 750 + (unique_suffix() % 1000) as u64;
    pg.set_reference_gas_price(price).await.unwrap();
    let fetched = pg.get_reference_gas_price().await.unwrap().unwrap();
    assert_eq!(fetched, price);
}
