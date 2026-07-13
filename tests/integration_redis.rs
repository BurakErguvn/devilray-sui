#![cfg(feature = "integration")]

use devilray_sui::models::{PoolState, PoolTickData, TickInfo, Token};
use devilray_sui::queue::{
    DlqEntry, MessageQueueTrait, QueueMessage, RedisMessageQueue, replay_dlq,
};
use devilray_sui::storage::redis::RedisCache;
use devilray_sui::storage::{RedisCacheTrait, SwapEvent};
use std::env;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

async fn connect_redis() -> Option<(RedisCache, RedisMessageQueue)> {
    let redis_url = env::var("REDIS_URL").ok()?;
    let client = redis::Client::open(redis_url).ok()?;
    let conn = client.get_multiplexed_async_connection().await.ok()?;
    let cache = RedisCache::new(client.clone(), conn.clone());
    let queue = RedisMessageQueue::new(conn);
    Some((cache, queue))
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}

#[tokio::test]
async fn redis_pool_state_round_trip() {
    let Some((cache, _queue)) = connect_redis().await else {
        return;
    };
    let suffix = unique_suffix();
    let pool = PoolState {
        pool_id: format!("redis_pool_{suffix}"),
        dex_name: "Cetus".to_string(),
        coin_type_a: "A".to_string(),
        coin_type_b: "B".to_string(),
        sqrt_price: 999,
        liquidity: 111,
        fee_rate: 2500,
        is_paused: false,
    };

    cache.set_pool_state(&pool).await.unwrap();
    let fetched = cache.get_pool_state(&pool.pool_id).await.unwrap().unwrap();
    assert_eq!(fetched.sqrt_price, pool.sqrt_price);
}

#[tokio::test]
async fn redis_active_pools_round_trip() {
    let Some((cache, _queue)) = connect_redis().await else {
        return;
    };
    let suffix = unique_suffix();
    let pools = vec![PoolState {
        pool_id: format!("active_pool_{suffix}"),
        dex_name: "Turbos".to_string(),
        coin_type_a: "X".to_string(),
        coin_type_b: "Y".to_string(),
        sqrt_price: 1,
        liquidity: 2,
        fee_rate: 3000,
        is_paused: false,
    }];

    cache.set_active_pools(&pools).await.unwrap();
    let fetched = cache.get_active_pools().await.unwrap().unwrap();
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].pool_id, pools[0].pool_id);
}

#[tokio::test]
async fn redis_all_tokens_round_trip() {
    let Some((cache, _queue)) = connect_redis().await else {
        return;
    };
    let suffix = unique_suffix();
    let tokens = vec![Token {
        address: format!("redis_token_{suffix}"),
        symbol: "RT".to_string(),
        name: "Redis Token".to_string(),
        decimals: 9,
    }];

    cache.set_all_tokens(&tokens).await.unwrap();
    let fetched = cache.get_all_tokens().await.unwrap().unwrap();
    assert_eq!(fetched[0].address, tokens[0].address);
}

#[tokio::test]
async fn redis_reference_gas_price_round_trip() {
    let Some((cache, _queue)) = connect_redis().await else {
        return;
    };
    let price = 500 + (unique_suffix() % 500) as u64;
    cache.set_reference_gas_price(price).await.unwrap();
    let fetched = cache.get_reference_gas_price().await.unwrap().unwrap();
    assert_eq!(fetched, price);
}

#[tokio::test]
async fn redis_tick_data_round_trip() {
    let Some((cache, _queue)) = connect_redis().await else {
        return;
    };
    let suffix = unique_suffix();
    let tick_data = PoolTickData {
        pool_id: format!("redis_ticks_{suffix}"),
        current_tick_index: 7,
        tick_spacing: 10,
        ticks: vec![TickInfo {
            tick_index: 10,
            liquidity_net: 500,
        }],
        ..Default::default()
    };

    cache.set_pool_tick_data(&tick_data).await.unwrap();
    let fetched = cache
        .get_pool_tick_data(&tick_data.pool_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.current_tick_index, 7);
}

#[tokio::test]
async fn redis_queue_publish_consume_round_trip() {
    let Some((_cache, queue)) = connect_redis().await else {
        return;
    };
    let suffix = unique_suffix();
    let queue_name = format!("test_queue_{suffix}");
    let msg = QueueMessage::PoolStateUpdate(PoolState {
        pool_id: format!("q_pool_{suffix}"),
        dex_name: "Cetus".to_string(),
        coin_type_a: "A".to_string(),
        coin_type_b: "B".to_string(),
        sqrt_price: 1,
        liquidity: 2,
        fee_rate: 2500,
        is_paused: false,
    });

    queue.publish(&queue_name, &msg).await.unwrap();
    let consumed = queue.consume(&queue_name).await.unwrap().unwrap();
    assert_eq!(consumed, msg);
}

#[tokio::test]
async fn redis_dlq_push_pop_len_and_replay() {
    let Some((_cache, queue)) = connect_redis().await else {
        return;
    };
    let suffix = unique_suffix();
    let main_queue = format!("main_q_{suffix}");
    let dlq_queue = format!("dlq_q_{suffix}");
    let queue: Arc<dyn MessageQueueTrait> = Arc::new(queue);

    let msg = QueueMessage::SwapEventLog(SwapEvent {
        event_id: format!("evt_{suffix}"),
        timestamp: 1,
        pool_id: "pool".to_string(),
        dex_name: "Cetus".to_string(),
        sender: "0x1".to_string(),
        amount_in: "100".to_string(),
        amount_out: "99".to_string(),
        coin_in: "A".to_string(),
        coin_out: "B".to_string(),
    });
    let entry = DlqEntry {
        message: msg.clone(),
        failure_reason: "test".to_string(),
        failed_at_unix: 1,
        attempts: 3,
    };

    queue.push_dlq(&dlq_queue, &entry).await.unwrap();
    assert_eq!(queue.dlq_len(&dlq_queue).await.unwrap(), 1);

    let replayed = replay_dlq(queue.as_ref(), &main_queue, &dlq_queue, 10)
        .await
        .unwrap();
    assert_eq!(replayed, 1);
    assert_eq!(queue.dlq_len(&dlq_queue).await.unwrap(), 0);

    let consumed = queue.consume(&main_queue).await.unwrap().unwrap();
    assert_eq!(consumed, msg);
}
