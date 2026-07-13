use crate::api::info::load_tokens_map;
use crate::api::metrics::{self, quote_timer};
use crate::api::websocket::ServerAppState;
use crate::models::{
    BuildTxRequest, BuildTxResponse, PoolState, PoolTickData, PtbArgument, PtbCommand,
    PtbTransaction, QuoteRequest, QuoteResponse, RouteStep,
};
use crate::router::{RouteConfig, TokenGraph};
use crate::storage::{PostgresStorageTrait, RedisCacheTrait};
use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

pub async fn handle_quote(
    State(state): State<ServerAppState>,
    Query(payload): Query<QuoteRequest>,
) -> Result<impl IntoResponse, impl IntoResponse> {
    let started = quote_timer();
    match handle_quote_inner(state, payload).await {
        Ok((response, active_count)) => {
            metrics::record_quote_ok(started.elapsed(), active_count as i64);
            Ok(Json(response))
        }
        Err(err) => {
            metrics::record_quote_error(started.elapsed());
            Err(err)
        }
    }
}

async fn handle_quote_inner(
    state: ServerAppState,
    payload: QuoteRequest,
) -> Result<(QuoteResponse, usize), (StatusCode, Json<ErrorResponse>)> {
    let baseline = load_active_pools(&state).await?;
    if baseline.is_empty()
        && !state
            .topology_ready
            .load(std::sync::atomic::Ordering::SeqCst)
    {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "topology not ready: pool discovery bootstrap incomplete".to_string(),
            }),
        ));
    }
    let active_count = baseline.len();

    let mut compiled_pools = Vec::with_capacity(baseline.len());
    for mut pool in baseline {
        if let Ok(Some(hot_state)) = state.redis_cache.get_pool_state(&pool.pool_id).await {
            pool = hot_state;
        }
        if pool.is_paused {
            continue;
        }
        compiled_pools.push(pool);
    }

    let amount_in = match payload.amount.parse::<u128>() {
        Ok(val) => val,
        Err(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Invalid amount: must be a valid u128 integer".to_string(),
                }),
            ));
        }
    };

    let mut graph = TokenGraph::new();
    graph.build_from_pools(&compiled_pools);

    let config = RouteConfig::default();
    let routes = graph.find_best_route(&payload.from_token, &payload.to_token, amount_in, &config);

    if routes.is_empty() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "No route found between the specified tokens".to_string(),
            }),
        ));
    }

    let pools_map: HashMap<String, PoolState> = compiled_pools
        .into_iter()
        .map(|p| (p.pool_id.clone(), p))
        .collect();

    let mut decimals_map = load_tokens_map(&state).await;
    for pool in pools_map.values() {
        decimals_map.entry(pool.coin_type_a.clone()).or_insert(9);
        decimals_map.entry(pool.coin_type_b.clone()).or_insert(9);
    }

    let ticks_map =
        load_ticks_map(&pools_map, state.redis_cache.as_ref(), state.pg_db.as_ref()).await;

    let reference_gas_price = match state.redis_cache.get_reference_gas_price().await {
        Ok(Some(price)) => price,
        _ => match state.pg_db.get_reference_gas_price().await {
            Ok(Some(price)) => price,
            _ => 1000,
        },
    };

    let splits = crate::router::optimize_order_split(
        &routes,
        amount_in,
        &payload.to_token,
        reference_gas_price,
        &pools_map,
        &decimals_map,
        &ticks_map,
    );

    if splits.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Failed to optimize route split".to_string(),
            }),
        ));
    }

    let amount_out: u128 = splits.iter().map(|s| s.2).sum();

    let mut price_impact = 0.0;
    let mut route_steps = Vec::new();

    for (r, alloc_in, _) in &splits {
        let pct = ((*alloc_in as f64) / (amount_in as f64) * 100.0) as u32;
        if pct == 0 {
            continue;
        }

        // Multiplicative price impact using tick-aware / within-tick simulation
        let mut path_survival = 1.0;
        let mut curr_alloc_in = *alloc_in as f64;
        for hop in &r.hops {
            if let Some(pool) = pools_map.get(&hop.pool_id) {
                let (_, spot_m) = hop_simulate_output(pool, hop, 0.0, &ticks_map, &decimals_map);
                let (out_actual, _) =
                    hop_simulate_output(pool, hop, curr_alloc_in, &ticks_map, &decimals_map);
                let ideal_out = curr_alloc_in * spot_m;
                let hop_impact = if ideal_out > 0.0 {
                    (1.0 - out_actual / ideal_out).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                path_survival *= 1.0 - hop_impact;
                curr_alloc_in = out_actual;
            }
        }
        let path_impact = 1.0 - path_survival;
        price_impact += path_impact * ((*alloc_in as f64) / (amount_in as f64));

        for hop in &r.hops {
            route_steps.push(RouteStep {
                dex_name: hop.dex_name.clone(),
                pool_address: hop.pool_id.clone(),
                weight: pct,
            });
        }
    }

    let response = QuoteResponse {
        from_token: payload.from_token,
        to_token: payload.to_token,
        amount_in: payload.amount,
        amount_out: amount_out.to_string(),
        price_impact,
        route: route_steps,
    };

    Ok((response, active_count))
}

pub async fn handle_build_tx(
    State(state): State<ServerAppState>,
    Json(payload): Json<BuildTxRequest>,
) -> Result<Json<BuildTxResponse>, (StatusCode, Json<ErrorResponse>)> {
    let amount_in = match payload.amount.parse::<u128>() {
        Ok(val) => val,
        Err(_) => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Invalid input amount format".to_string(),
                }),
            ));
        }
    };

    let baseline = match load_active_pools(&state).await {
        Ok(pools) => pools,
        Err(err) => return Err(err),
    };

    let mut compiled_pools = Vec::new();
    for mut pool in baseline {
        if let Ok(Some(cached_state)) = state.redis_cache.get_pool_state(&pool.pool_id).await {
            pool = cached_state;
        }
        if pool.is_paused {
            continue;
        }
        compiled_pools.push(pool);
    }

    let mut graph = TokenGraph::new();
    graph.build_from_pools(&compiled_pools);

    let config = RouteConfig::default();
    let routes = graph.find_best_route(&payload.from_token, &payload.to_token, amount_in, &config);

    if routes.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "No routes found for the requested swap".to_string(),
            }),
        ));
    }

    let pools_map: HashMap<String, PoolState> = compiled_pools
        .into_iter()
        .map(|p| (p.pool_id.clone(), p))
        .collect();

    let mut decimals_map = load_tokens_map(&state).await;
    for pool in pools_map.values() {
        decimals_map.entry(pool.coin_type_a.clone()).or_insert(9);
        decimals_map.entry(pool.coin_type_b.clone()).or_insert(9);
    }

    let ticks_map =
        load_ticks_map(&pools_map, state.redis_cache.as_ref(), state.pg_db.as_ref()).await;

    let reference_gas_price = match state.redis_cache.get_reference_gas_price().await {
        Ok(Some(price)) => price,
        _ => match state.pg_db.get_reference_gas_price().await {
            Ok(Some(price)) => price,
            _ => 1000,
        },
    };

    let splits = crate::router::optimize_order_split(
        &routes,
        amount_in,
        &payload.to_token,
        reference_gas_price,
        &pools_map,
        &decimals_map,
        &ticks_map,
    );

    if splits.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Failed to optimize route split".to_string(),
            }),
        ));
    }

    let slippage =
        match crate::slippage::SlippageBps::try_from_tolerance(payload.slippage_tolerance) {
            Ok(s) => s,
            Err(e) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                ));
            }
        };

    let total_amount_out: u128 = splits.iter().map(|s| s.2).sum();
    let min_amount_out = crate::slippage::path_min_amount_out(total_amount_out, &slippage);
    let from_token_norm = payload.from_token.to_lowercase();
    let is_from_sui = from_token_norm == "sui" || from_token_norm.contains("0x2::sui::sui");

    let mut commands = Vec::new();
    let mut split_amounts = Vec::new();
    for (_, alloc_in, _) in &splits {
        let amount_u64 = u64::try_from(*alloc_in).map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Split amount does not fit in u64".to_string(),
                }),
            )
        })?;
        split_amounts.push(PtbArgument::U64(amount_u64));
    }

    let (_input_coin_arg, split_cmd_idx) = if is_from_sui {
        commands.push(PtbCommand::SplitCoins {
            coin: PtbArgument::GasCoin,
            amounts: split_amounts,
        });
        (PtbArgument::GasCoin, 0)
    } else {
        let coin_ids_list = payload.coin_ids.clone().unwrap_or_default();
        if coin_ids_list.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "coin_ids required for non-SUI input swaps".to_string(),
                }),
            ));
        }
        if coin_ids_list.len() > 1 {
            let dest_coin = PtbArgument::Object(coin_ids_list[0].clone());
            let sources = coin_ids_list[1..]
                .iter()
                .map(|id| PtbArgument::Object(id.clone()))
                .collect();
            commands.push(PtbCommand::MergeCoins {
                destination: dest_coin.clone(),
                sources,
            });
            commands.push(PtbCommand::SplitCoins {
                coin: dest_coin,
                amounts: split_amounts,
            });
            (PtbArgument::InputCoin, 1)
        } else {
            let base_coin = PtbArgument::Object(coin_ids_list[0].clone());
            commands.push(PtbCommand::SplitCoins {
                coin: base_coin,
                amounts: split_amounts,
            });
            (PtbArgument::InputCoin, 0)
        }
    };

    let mut swap_output_args = Vec::new();

    for (i, (r, alloc_in, alloc_out)) in splits.iter().enumerate() {
        let _path_pct = (*alloc_in as f64) / (amount_in as f64);
        let path_min_out = slippage.apply_to_amount(*alloc_out);

        let mut held_coin = Some(PtbArgument::NestedResult(split_cmd_idx, i as u16));
        let mut expected_current_amount = *alloc_in;
        let total_hops = r.hops.len();

        for (hop_idx, hop) in r.hops.iter().enumerate() {
            let pool = pools_map.get(&hop.pool_id).unwrap();
            let is_a_to_b = hop.input_token == pool.coin_type_a;

            let sim = if let Some(td) = ticks_map.get(&hop.pool_id) {
                crate::router::simulate_swap_tick_aware_detailed(
                    pool,
                    td,
                    hop,
                    expected_current_amount as f64,
                    &decimals_map,
                )
            } else {
                crate::router::simulate_hop_within_tick_detailed(
                    pool,
                    hop,
                    expected_current_amount as f64,
                    &decimals_map,
                )
            };
            let next_expected_amount_u = sim.amount_out.max(0.0) as u128;

            let hop_min_out = if hop_idx + 1 == total_hops {
                path_min_out
            } else {
                crate::slippage::intermediate_hop_min_out(
                    next_expected_amount_u,
                    &slippage,
                    total_hops as u32,
                )
            };

            let adverse_bps = slippage.hop_adverse_bps(hop_idx, total_hops);
            let sqrt_limit = crate::slippage::dynamic_sqrt_price_limit_raw(
                sim.final_sqrt_price_internal,
                crate::router::pool_sqrt_factor_bits(pool),
                is_a_to_b,
                adverse_bps,
            );

            let start_cmd_idx = commands.len() as u16;
            let is_final_hop = hop_idx + 1 == total_hops;
            let current_coin = held_coin.take().ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: "route hop missing input coin".to_string(),
                    }),
                )
            })?;
            let (hop_cmds, new_coin) = match crate::dex_swap::build_hop_commands(
                pool,
                hop,
                current_coin,
                expected_current_amount,
                hop_min_out,
                sqrt_limit,
                start_cmd_idx,
                &payload.user_address,
                is_final_hop,
            ) {
                Ok(v) => v,
                Err(msg) => {
                    return Err((StatusCode::BAD_REQUEST, Json(ErrorResponse { error: msg })));
                }
            };
            commands.extend(hop_cmds);
            if new_coin.is_none() && !is_final_hop {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: "route hop produced no transferable coin mid-path".to_string(),
                    }),
                ));
            }
            held_coin = new_coin;
            expected_current_amount = next_expected_amount_u;
        }

        if let Some(coin) = held_coin {
            swap_output_args.push(coin);
        }
    }

    if !swap_output_args.is_empty() {
        commands.push(PtbCommand::TransferObjects {
            objects: swap_output_args,
            address: PtbArgument::Address(payload.user_address.clone()),
        });
    }

    let transaction = PtbTransaction {
        sender: payload.user_address.clone(),
        commands,
    };

    let gas_budget = std::env::var("BUILD_TX_GAS_BUDGET")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50_000_000u64);
    let gas_price = if reference_gas_price == 0 {
        1_000
    } else {
        reference_gas_price
    };

    let canonical = match finalize_canonical_build(
        state.sui_client.as_ref(),
        &transaction,
        &payload,
        amount_in,
        is_from_sui,
        gas_price,
        gas_budget,
    )
    .await
    {
        Ok(c) => c,
        Err(err) => {
            return Err((err.0, Json(ErrorResponse { error: err.1 })));
        }
    };

    Ok(Json(BuildTxResponse {
        transaction_data_bcs: canonical.transaction_data_bcs_base64,
        transaction_digest: canonical.transaction_digest,
        gas_budget: canonical.gas_budget,
        gas_price: canonical.gas_price,
        object_refs: canonical.object_refs,
        debug_transaction: transaction,
        estimated_amount_out: total_amount_out.to_string(),
        min_amount_out: min_amount_out.to_string(),
    }))
}

async fn finalize_canonical_build(
    sui: &dyn crate::sui_client::SuiClientTrait,
    symbolic: &PtbTransaction,
    payload: &BuildTxRequest,
    amount_in: u128,
    is_from_sui: bool,
    gas_price: u64,
    gas_budget: u64,
) -> Result<crate::transaction_builder::CanonicalBuildOutput, (StatusCode, String)> {
    use crate::transaction_builder::{
        CanonicalBuildInput, ObjectMeta, ResolvedCoin, build_canonical_transaction,
    };
    use std::collections::{HashMap, HashSet};

    let mut object_ids = HashSet::new();
    collect_object_ids_from_ptb(symbolic, &mut object_ids);

    let mut object_meta: HashMap<String, ObjectMeta> = HashMap::new();
    for object_id in &object_ids {
        let meta = sui.get_object_meta(object_id).await.map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("failed to resolve object {object_id}: {e}"),
            )
        })?;
        object_meta.insert(object_id.clone(), meta);
    }

    let sui_coins = sui
        .get_coins(
            payload.user_address.as_str(),
            Some("0x2::sui::SUI"),
            None,
            None,
        )
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("failed to fetch SUI coins for gas: {e}"),
            )
        })?;

    let mut gas_coins: Vec<ResolvedCoin> = Vec::new();
    let remaining_for_input = amount_in;
    if is_from_sui {
        // Prefer a gas coin that can cover amount_in + gas budget; fall back to largest.
        let mut sorted = sui_coins;
        sorted.sort_by_key(|c| std::cmp::Reverse(c.balance));
        let needed = amount_in
            .saturating_add(u128::from(gas_budget))
            .min(u128::from(u64::MAX)) as u64;
        let chosen = sorted
            .iter()
            .find(|c| c.balance >= needed)
            .cloned()
            .or_else(|| sorted.first().cloned())
            .ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    "sender has no SUI coins for gas/payment".to_string(),
                )
            })?;
        if u128::from(chosen.balance) < amount_in {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "insufficient SUI balance: have {}, need {}",
                    chosen.balance, amount_in
                ),
            ));
        }
        gas_coins.push(chosen);
    } else {
        let chosen = sui_coins
            .into_iter()
            .max_by_key(|c| c.balance)
            .ok_or_else(|| {
                (
                    StatusCode::BAD_REQUEST,
                    "sender has no SUI coins for gas payment".to_string(),
                )
            })?;
        if chosen.balance < gas_budget {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "insufficient SUI for gas budget: have {}, need at least {}",
                    chosen.balance, gas_budget
                ),
            ));
        }
        gas_coins.push(chosen);

        let coin_ids = payload.coin_ids.clone().unwrap_or_default();
        let mut input_total = 0u128;
        for coin_id in &coin_ids {
            let meta = if let Some(m) = object_meta.get(coin_id) {
                m.clone()
            } else {
                let m = sui.get_object_meta(coin_id).await.map_err(|e| {
                    (
                        StatusCode::BAD_GATEWAY,
                        format!("failed to resolve input coin {coin_id}: {e}"),
                    )
                })?;
                object_meta.insert(coin_id.clone(), m.clone());
                m
            };
            // Balance is not in ObjectMeta; fetch via get_coins filtered by id.
            let owned = sui
                .get_coins(payload.user_address.as_str(), None, None, None)
                .await
                .map_err(|e| {
                    (
                        StatusCode::BAD_GATEWAY,
                        format!("failed to list sender coins: {e}"),
                    )
                })?;
            let coin = owned
                .into_iter()
                .find(|c| c.object_id == *coin_id)
                .ok_or_else(|| {
                    (
                        StatusCode::BAD_REQUEST,
                        format!("coin_id {coin_id} not owned by sender"),
                    )
                })?;
            input_total = input_total.saturating_add(u128::from(coin.balance));
            let _ = meta; // ownership already validated via get_coins
            let _ = remaining_for_input;
        }
        if input_total < amount_in {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("insufficient input coin balance: have {input_total}, need {amount_in}"),
            ));
        }
    }

    let input = CanonicalBuildInput {
        symbolic: symbolic.clone(),
        sender: payload.user_address.clone(),
        gas_price,
        gas_budget,
        gas_coins,
        input_coins: HashMap::new(),
        object_meta,
    };

    build_canonical_transaction(&input).map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            format!("failed to build canonical TransactionData: {e}"),
        )
    })
}

fn collect_object_ids_from_ptb(tx: &PtbTransaction, out: &mut std::collections::HashSet<String>) {
    for cmd in &tx.commands {
        match cmd {
            PtbCommand::SplitCoins { coin, amounts } => {
                collect_object_ids_from_arg(coin, out);
                for a in amounts {
                    collect_object_ids_from_arg(a, out);
                }
            }
            PtbCommand::MergeCoins {
                destination,
                sources,
            } => {
                collect_object_ids_from_arg(destination, out);
                for s in sources {
                    collect_object_ids_from_arg(s, out);
                }
            }
            PtbCommand::TransferObjects { objects, address } => {
                for o in objects {
                    collect_object_ids_from_arg(o, out);
                }
                collect_object_ids_from_arg(address, out);
            }
            PtbCommand::MoveCall { arguments, .. } => {
                for a in arguments {
                    collect_object_ids_from_arg(a, out);
                }
            }
            PtbCommand::MakeMoveVec { elements, .. } => {
                for e in elements {
                    collect_object_ids_from_arg(e, out);
                }
            }
        }
    }
}

fn collect_object_ids_from_arg(arg: &PtbArgument, out: &mut std::collections::HashSet<String>) {
    match arg {
        PtbArgument::Object(id) => {
            out.insert(id.clone());
        }
        PtbArgument::Pure(s) if s.starts_with("0x") && s.len() >= 3 => {
            // legacy object-as-pure
            out.insert(s.clone());
        }
        _ => {}
    }
}

async fn load_active_pools(
    state: &ServerAppState,
) -> Result<Vec<PoolState>, (StatusCode, Json<ErrorResponse>)> {
    if let Ok(Some(pools)) = state.redis_cache.get_active_pools().await {
        return Ok(pools);
    }
    let pools = state.pg_db.list_pools().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to query pools from database: {e}"),
            }),
        )
    })?;
    let active: Vec<PoolState> = pools.into_iter().filter(|p| !p.is_paused).collect();
    let _ = state.redis_cache.set_active_pools(&active).await;
    Ok(active)
}

async fn load_ticks_map(
    pools_map: &HashMap<String, PoolState>,
    redis_cache: &dyn RedisCacheTrait,
    pg_db: &dyn PostgresStorageTrait,
) -> HashMap<String, PoolTickData> {
    let mut ticks_map = HashMap::new();
    for pool_id in pools_map.keys() {
        if let Ok(Some(td)) = redis_cache.get_pool_tick_data(pool_id).await {
            ticks_map.insert(pool_id.clone(), td);
        } else if let Ok(Some(td)) = pg_db.get_pool_tick_data(pool_id).await {
            ticks_map.insert(pool_id.clone(), td);
        }
    }
    ticks_map
}

fn hop_simulate_output(
    pool: &PoolState,
    hop: &crate::router::SwapHop,
    amount_in: f64,
    ticks_map: &HashMap<String, PoolTickData>,
    decimals_map: &HashMap<String, u8>,
) -> (f64, f64) {
    if let Some(td) = ticks_map.get(&hop.pool_id) {
        crate::router::simulate_swap_tick_aware(pool, td, hop, amount_in, decimals_map)
    } else {
        crate::router::simulate_hop_within_tick(pool, hop, amount_in, decimals_map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Token;
    use crate::storage::PostgresStorageTrait;
    use crate::storage::postgres::tests::InMemoryPostgresStorage;
    use crate::storage::redis::tests::InMemoryRedisCache;
    use crate::sui_client::InMemorySuiClient;
    use crate::transaction_builder::{ObjectMeta, OwnerKind, ResolvedCoin};
    use axum::{Router, routing::get};
    use std::sync::Arc;
    use sui_crypto::ed25519::Ed25519PrivateKey;
    use sui_crypto::{SuiSigner, SuiVerifier};
    use sui_sdk_types::Digest;
    use sui_sdk_types::Transaction;
    use sui_sdk_types::bcs::{FromBcs, ToBcs};
    use tokio::net::TcpListener;
    use tokio::sync::broadcast;

    const POOL_CETUS: &str = "0xc1";
    const POOL_MOMENTUM: &str = "0xc2";
    const POOL_MAGMA: &str = "0xc3";
    const USER: &str = "0xb0b";
    const GAS_COIN: &str = "0xa1";
    const USDC_COIN1: &str = "0xd1";
    const USDC_COIN2: &str = "0xd2";
    const TYPE_SUI: &str = "0x2::sui::SUI";
    const TYPE_USDC: &str =
        "0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN";
    const TYPE_USDT: &str = "0xee::coin::COIN";

    fn sample_pools() -> Vec<PoolState> {
        vec![
            PoolState {
                pool_id: "pool_sui_usdc_cetus".to_string(),
                dex_name: "Cetus".to_string(),
                coin_type_a: "SUI".to_string(),
                coin_type_b: "USDC".to_string(),
                sqrt_price: 18446744073709551616, // Cetus 1:1 price (2^64)
                liquidity: 1000000000000,
                fee_rate: 3000,
                is_paused: false,
            },
            PoolState {
                pool_id: "pool_usdc_usdt_momentum".to_string(),
                dex_name: "Momentum".to_string(),
                coin_type_a: "USDC".to_string(),
                coin_type_b: "USDT".to_string(),
                sqrt_price: 18446744073709551616, // 1:1 price (2^64)
                liquidity: 2000000000000,
                fee_rate: 1000,
                is_paused: false,
            },
            PoolState {
                pool_id: "pool_sui_usdt_magma".to_string(),
                dex_name: "Magma".to_string(),
                coin_type_a: "SUI".to_string(),
                coin_type_b: "USDT".to_string(),
                sqrt_price: 18446744073709551616, // 1:1 price (2^64)
                liquidity: 10000000,
                fee_rate: 50000, // 5% fee (very high)
                is_paused: false,
            },
        ]
    }

    fn sample_pools_hex() -> Vec<PoolState> {
        vec![
            PoolState {
                pool_id: POOL_CETUS.to_string(),
                dex_name: "Cetus".to_string(),
                coin_type_a: TYPE_SUI.to_string(),
                coin_type_b: TYPE_USDC.to_string(),
                sqrt_price: 18446744073709551616,
                liquidity: 1000000000000,
                fee_rate: 3000,
                is_paused: false,
            },
            PoolState {
                pool_id: POOL_MOMENTUM.to_string(),
                dex_name: "Momentum".to_string(),
                coin_type_a: TYPE_USDC.to_string(),
                coin_type_b: TYPE_USDT.to_string(),
                sqrt_price: 18446744073709551616,
                liquidity: 2000000000000,
                fee_rate: 1000,
                is_paused: false,
            },
            PoolState {
                pool_id: POOL_MAGMA.to_string(),
                dex_name: "Magma".to_string(),
                coin_type_a: TYPE_SUI.to_string(),
                coin_type_b: TYPE_USDT.to_string(),
                sqrt_price: 18446744073709551616,
                liquidity: 10000000,
                fee_rate: 50000,
                is_paused: false,
            },
        ]
    }

    fn shared_meta(object_id: &str) -> ObjectMeta {
        ObjectMeta {
            object_id: object_id.to_string(),
            version: 1,
            digest: Digest::ZERO.to_base58(),
            owner_kind: OwnerKind::Shared,
            initial_shared_version: Some(1),
            mutable: true,
        }
    }

    fn owned_meta(object_id: &str) -> ObjectMeta {
        ObjectMeta {
            object_id: object_id.to_string(),
            version: 1,
            digest: Digest::ZERO.to_base58(),
            owner_kind: OwnerKind::Owned,
            initial_shared_version: None,
            mutable: false,
        }
    }

    fn in_memory_sui_for_build_tx() -> Arc<InMemorySuiClient> {
        let client = Arc::new(InMemorySuiClient::new(1_000));
        for object_id in [
            GAS_COIN,
            USDC_COIN1,
            USDC_COIN2,
            POOL_CETUS,
            POOL_MOMENTUM,
            POOL_MAGMA,
            crate::discovery::registry::SUI_CLOCK,
            crate::discovery::registry::CETUS_GLOBAL_CONFIG,
            crate::discovery::registry::MAGMA_GLOBAL_CONFIG,
            crate::discovery::registry::MOMENTUM_VERSION_OBJECT,
        ] {
            let meta = if [GAS_COIN, USDC_COIN1, USDC_COIN2].contains(&object_id) {
                owned_meta(object_id)
            } else {
                shared_meta(object_id)
            };
            client.insert_object_meta(meta);
        }
        let digest = Digest::ZERO.to_base58();
        for coin in [
            ResolvedCoin {
                object_id: GAS_COIN.to_string(),
                version: 1,
                digest: digest.clone(),
                balance: 10_000_000_000,
                coin_type: TYPE_SUI.to_string(),
            },
            ResolvedCoin {
                object_id: USDC_COIN1.to_string(),
                version: 1,
                digest: digest.clone(),
                balance: 5_000_000,
                coin_type: TYPE_USDC.to_string(),
            },
            ResolvedCoin {
                object_id: USDC_COIN2.to_string(),
                version: 1,
                digest,
                balance: 5_000_000,
                coin_type: TYPE_USDC.to_string(),
            },
        ] {
            client.insert_coin(coin);
        }
        client
    }

    async fn seed_tokens(pg_db: &InMemoryPostgresStorage) {
        for (addr, decimals) in [
            (TYPE_SUI, 9u8),
            (TYPE_USDC, 6),
            (TYPE_USDT, 6),
            ("SUI", 9),
            ("USDC", 6),
            ("USDT", 6),
        ] {
            pg_db
                .insert_token(&Token {
                    address: addr.to_string(),
                    symbol: addr.to_string(),
                    name: addr.to_string(),
                    decimals,
                })
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn test_quote_endpoint_dijkstra() {
        let pg_db = Arc::new(InMemoryPostgresStorage::new());
        let redis_cache = Arc::new(InMemoryRedisCache::new());
        let (broadcast_tx, _) = broadcast::channel(10);

        // Add sample pools and tokens to DB
        pg_db
            .insert_token(&Token {
                address: "SUI".to_string(),
                symbol: "SUI".to_string(),
                name: "Sui".to_string(),
                decimals: 9,
            })
            .await
            .unwrap();

        pg_db
            .insert_token(&Token {
                address: "USDC".to_string(),
                symbol: "USDC".to_string(),
                name: "USDC".to_string(),
                decimals: 6,
            })
            .await
            .unwrap();

        pg_db
            .insert_token(&Token {
                address: "USDT".to_string(),
                symbol: "USDT".to_string(),
                name: "USDT".to_string(),
                decimals: 6,
            })
            .await
            .unwrap();

        for pool in sample_pools() {
            pg_db.insert_pool(&pool).await.unwrap();
        }

        let app_state = ServerAppState::new(
            broadcast_tx,
            pg_db,
            redis_cache,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            Arc::new(crate::sui_client::tests::MockSuiClient::new()),
        );
        let app = Router::new()
            .route("/api/quote", get(handle_quote))
            .with_state(app_state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Test small amount Dijkstra route SUI -> USDT (2 hops: SUI -> USDC -> USDT)
        let client = reqwest::Client::new();
        let url = format!(
            "http://{}/api/quote?from_token=SUI&to_token=USDT&amount=1000000",
            addr
        );
        let res = client.get(&url).send().await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let quote: QuoteResponse = res.json().await.unwrap();
        assert_eq!(quote.from_token, "SUI");
        assert_eq!(quote.to_token, "USDT");
        assert_eq!(quote.amount_in, "1000000");
        assert_eq!(quote.route.len(), 2);
        assert_eq!(quote.route[0].pool_address, "pool_sui_usdc_cetus");
        assert_eq!(quote.route[1].pool_address, "pool_usdc_usdt_momentum");
    }

    #[tokio::test]
    async fn test_quote_endpoint_dfs() {
        let pg_db = Arc::new(InMemoryPostgresStorage::new());
        let redis_cache = Arc::new(InMemoryRedisCache::new());
        let (broadcast_tx, _) = broadcast::channel(10);

        pg_db
            .insert_token(&Token {
                address: "SUI".to_string(),
                symbol: "SUI".to_string(),
                name: "Sui".to_string(),
                decimals: 9,
            })
            .await
            .unwrap();

        pg_db
            .insert_token(&Token {
                address: "USDC".to_string(),
                symbol: "USDC".to_string(),
                name: "USDC".to_string(),
                decimals: 6,
            })
            .await
            .unwrap();

        pg_db
            .insert_token(&Token {
                address: "USDT".to_string(),
                symbol: "USDT".to_string(),
                name: "USDT".to_string(),
                decimals: 6,
            })
            .await
            .unwrap();

        for pool in sample_pools() {
            pg_db.insert_pool(&pool).await.unwrap();
        }

        let app_state = ServerAppState::new(
            broadcast_tx,
            pg_db,
            redis_cache,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            Arc::new(crate::sui_client::tests::MockSuiClient::new()),
        );
        let app = Router::new()
            .route("/api/quote", get(handle_quote))
            .with_state(app_state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Test large amount DFS route SUI -> USDT (amount = 200,000,000, which is > 100,000,000 threshold)
        // DFS will yield both SUI -> USDT (Magma) and SUI -> USDC -> USDT (Cetus + Momentum)
        // Simulator should choose the one with the higher output amount (which is Cetus + Momentum because fee is 0.4% vs 5%)
        let client = reqwest::Client::new();
        let url = format!(
            "http://{}/api/quote?from_token=SUI&to_token=USDT&amount=200000000",
            addr
        );
        let res = client.get(&url).send().await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let quote: QuoteResponse = res.json().await.unwrap();
        assert_eq!(quote.from_token, "SUI");
        assert_eq!(quote.to_token, "USDT");
        assert_eq!(quote.amount_in, "200000000");
        assert_eq!(quote.route.len(), 2);
        assert_eq!(quote.route[0].pool_address, "pool_sui_usdc_cetus");
        assert_eq!(quote.route[1].pool_address, "pool_usdc_usdt_momentum");
    }

    #[tokio::test]
    async fn test_build_tx_endpoint() {
        let pg_db = Arc::new(InMemoryPostgresStorage::new());
        let redis_cache = Arc::new(InMemoryRedisCache::new());
        let (broadcast_tx, _) = broadcast::channel(10);
        seed_tokens(&pg_db).await;
        for pool in sample_pools_hex() {
            pg_db.insert_pool(&pool).await.unwrap();
        }

        let app_state = ServerAppState::new(
            broadcast_tx,
            pg_db,
            redis_cache,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            in_memory_sui_for_build_tx(),
        );
        let app = Router::new()
            .route("/api/build_tx", axum::routing::post(handle_build_tx))
            .with_state(app_state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::new();
        let payload = BuildTxRequest {
            from_token: TYPE_SUI.to_string(),
            to_token: TYPE_USDT.to_string(),
            amount: "1000000000".to_string(),
            user_address: USER.to_string(),
            slippage_tolerance: 0.01,
            coin_ids: None,
        };

        let res = client
            .post(format!("http://{}/api/build_tx", addr))
            .json(&payload)
            .send()
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::OK);

        let tx_resp: BuildTxResponse = res.json().await.unwrap();
        assert_eq!(tx_resp.debug_transaction.sender, USER);
        assert!(!tx_resp.debug_transaction.commands.is_empty());
        assert!(!tx_resp.transaction_data_bcs.is_empty());
        assert!(!tx_resp.transaction_digest.is_empty());
        assert!(tx_resp.gas_budget > 0);

        let canonical = Transaction::from_bcs_base64(&tx_resp.transaction_data_bcs).unwrap();
        assert_eq!(
            canonical.to_bcs_base64().unwrap(),
            tx_resp.transaction_data_bcs,
            "canonical BCS must round-trip exactly"
        );
        assert_eq!(
            canonical.digest().to_base58(),
            tx_resp.transaction_digest,
            "response digest must be derived from canonical bytes"
        );
        let key = Ed25519PrivateKey::new([7; Ed25519PrivateKey::LENGTH]);
        let signature = key.sign_transaction(&canonical).unwrap();
        key.verifying_key()
            .verify_transaction(&canonical, &signature)
            .unwrap();

        match &tx_resp.debug_transaction.commands[0] {
            PtbCommand::SplitCoins { coin, amounts } => {
                assert_eq!(*coin, PtbArgument::GasCoin);
                assert!(!amounts.is_empty());
            }
            _ => panic!("First command is not SplitCoins"),
        }

        let last_cmd = tx_resp.debug_transaction.commands.last().unwrap();
        match last_cmd {
            PtbCommand::TransferObjects { objects, address } => {
                assert!(!objects.is_empty());
                assert_eq!(*address, PtbArgument::Address(USER.to_string()));
            }
            _ => panic!("Last command is not TransferObjects"),
        }
    }

    #[tokio::test]
    async fn canonical_build_fails_closed_on_missing_object_metadata() {
        let client = InMemorySuiClient::new(1_000);
        client.insert_coin(ResolvedCoin {
            object_id: GAS_COIN.to_string(),
            version: 1,
            digest: Digest::ZERO.to_base58(),
            balance: 1_000_000_000,
            coin_type: TYPE_SUI.to_string(),
        });
        let symbolic = PtbTransaction {
            sender: USER.to_string(),
            commands: vec![PtbCommand::MoveCall {
                package: crate::discovery::registry::CETUS_SWAP_PACKAGE.to_string(),
                module: "pool".to_string(),
                function: "flash_swap".to_string(),
                type_arguments: vec![TYPE_SUI.to_string(), TYPE_USDC.to_string()],
                arguments: vec![PtbArgument::Object(POOL_CETUS.to_string())],
            }],
        };
        let payload = BuildTxRequest {
            from_token: TYPE_SUI.to_string(),
            to_token: TYPE_USDC.to_string(),
            amount: "1000".to_string(),
            user_address: USER.to_string(),
            slippage_tolerance: 0.01,
            coin_ids: None,
        };

        let err =
            finalize_canonical_build(&client, &symbolic, &payload, 1_000, true, 1_000, 50_000_000)
                .await
                .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_GATEWAY);
        assert!(err.1.contains("missing"));
    }

    #[tokio::test]
    async fn canonical_build_fails_closed_without_gas_coin() {
        let client = InMemorySuiClient::new(1_000);
        let symbolic = PtbTransaction {
            sender: USER.to_string(),
            commands: vec![],
        };
        let payload = BuildTxRequest {
            from_token: TYPE_SUI.to_string(),
            to_token: TYPE_USDC.to_string(),
            amount: "1000".to_string(),
            user_address: USER.to_string(),
            slippage_tolerance: 0.01,
            coin_ids: None,
        };

        let err =
            finalize_canonical_build(&client, &symbolic, &payload, 1_000, true, 1_000, 50_000_000)
                .await
                .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("no SUI coins"));
    }

    #[tokio::test]
    async fn test_build_tx_endpoint_non_sui_merge() {
        let pg_db = Arc::new(InMemoryPostgresStorage::new());
        let redis_cache = Arc::new(InMemoryRedisCache::new());
        let (broadcast_tx, _) = broadcast::channel(10);
        seed_tokens(&pg_db).await;
        for pool in sample_pools_hex() {
            pg_db.insert_pool(&pool).await.unwrap();
        }

        let app_state = ServerAppState::new(
            broadcast_tx,
            pg_db,
            redis_cache,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            in_memory_sui_for_build_tx(),
        );
        let app = Router::new()
            .route("/api/build_tx", axum::routing::post(handle_build_tx))
            .with_state(app_state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::new();
        let payload = BuildTxRequest {
            from_token: TYPE_USDC.to_string(),
            to_token: TYPE_USDT.to_string(),
            amount: "1000000".to_string(),
            user_address: USER.to_string(),
            slippage_tolerance: 0.01,
            coin_ids: Some(vec![USDC_COIN1.to_string(), USDC_COIN2.to_string()]),
        };

        let res = client
            .post(format!("http://{}/api/build_tx", addr))
            .json(&payload)
            .send()
            .await
            .unwrap();

        assert_eq!(res.status(), StatusCode::OK);

        let tx_resp: BuildTxResponse = res.json().await.unwrap();
        assert_eq!(tx_resp.debug_transaction.sender, USER);
        assert!(!tx_resp.transaction_data_bcs.is_empty());

        match &tx_resp.debug_transaction.commands[0] {
            PtbCommand::MergeCoins {
                destination,
                sources,
            } => {
                assert_eq!(*destination, PtbArgument::Object(USDC_COIN1.to_string()));
                assert_eq!(sources.len(), 1);
                assert_eq!(sources[0], PtbArgument::Object(USDC_COIN2.to_string()));
            }
            _ => panic!("First command is not MergeCoins"),
        }

        match &tx_resp.debug_transaction.commands[1] {
            PtbCommand::SplitCoins { coin, amounts } => {
                assert_eq!(*coin, PtbArgument::Object(USDC_COIN1.to_string()));
                assert!(!amounts.is_empty());
            }
            _ => panic!("Second command is not SplitCoins"),
        }
    }

    #[tokio::test]
    async fn test_quote_uses_active_pools_cache() {
        use std::sync::atomic::Ordering;

        let pg_db = Arc::new(InMemoryPostgresStorage::new());
        let redis_cache = Arc::new(InMemoryRedisCache::new());
        let (broadcast_tx, _) = broadcast::channel(10);

        for token in ["SUI", "USDC", "USDT"] {
            pg_db
                .insert_token(&Token {
                    address: token.to_string(),
                    symbol: token.to_string(),
                    name: token.to_string(),
                    decimals: if token == "SUI" { 9 } else { 6 },
                })
                .await
                .unwrap();
        }
        for pool in sample_pools() {
            pg_db.insert_pool(&pool).await.unwrap();
        }

        let app_state = ServerAppState::new(
            broadcast_tx,
            pg_db.clone(),
            redis_cache,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            Arc::new(crate::sui_client::tests::MockSuiClient::new()),
        );
        let app = Router::new()
            .route("/api/quote", get(handle_quote))
            .with_state(app_state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::new();
        let url = format!(
            "http://{}/api/quote?from_token=SUI&to_token=USDT&amount=1000000",
            addr
        );

        let res = client.get(&url).send().await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(pg_db.list_pools_calls.load(Ordering::SeqCst), 1);

        let res2 = client.get(&url).send().await.unwrap();
        assert_eq!(res2.status(), StatusCode::OK);
        assert_eq!(pg_db.list_pools_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_build_tx_rejects_invalid_slippage() {
        let pg_db = Arc::new(InMemoryPostgresStorage::new());
        let redis_cache = Arc::new(InMemoryRedisCache::new());
        let (broadcast_tx, _) = broadcast::channel(10);
        seed_tokens(&pg_db).await;
        for pool in sample_pools_hex() {
            pg_db.insert_pool(&pool).await.unwrap();
        }

        let app_state = ServerAppState::new(
            broadcast_tx,
            pg_db,
            redis_cache,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            in_memory_sui_for_build_tx(),
        );
        let app = Router::new()
            .route("/api/build_tx", axum::routing::post(handle_build_tx))
            .with_state(app_state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::new();
        for bad_tol in [-0.01, 1.0] {
            let payload = BuildTxRequest {
                from_token: TYPE_SUI.to_string(),
                to_token: TYPE_USDT.to_string(),
                amount: "1000000000".to_string(),
                user_address: USER.to_string(),
                slippage_tolerance: bad_tol,
                coin_ids: None,
            };
            let res = client
                .post(format!("http://{}/api/build_tx", addr))
                .json(&payload)
                .send()
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn test_build_tx_min_out_uses_bps_not_f64_hack() {
        let pg_db = Arc::new(InMemoryPostgresStorage::new());
        let redis_cache = Arc::new(InMemoryRedisCache::new());
        let (broadcast_tx, _) = broadcast::channel(10);
        seed_tokens(&pg_db).await;
        for pool in sample_pools_hex() {
            pg_db.insert_pool(&pool).await.unwrap();
        }

        let app_state = ServerAppState::new(
            broadcast_tx,
            pg_db,
            redis_cache,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            in_memory_sui_for_build_tx(),
        );
        let app = Router::new()
            .route("/api/build_tx", axum::routing::post(handle_build_tx))
            .with_state(app_state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::new();
        let payload = BuildTxRequest {
            from_token: TYPE_SUI.to_string(),
            to_token: TYPE_USDT.to_string(),
            amount: "1000000000".to_string(),
            user_address: USER.to_string(),
            slippage_tolerance: 0.01,
            coin_ids: None,
        };

        let res = client
            .post(format!("http://{}/api/build_tx", addr))
            .json(&payload)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let tx_resp: BuildTxResponse = res.json().await.unwrap();
        let est_out: u128 = tx_resp.estimated_amount_out.parse().unwrap();
        let min_out: u128 = tx_resp.min_amount_out.parse().unwrap();
        let expected_min = est_out * 9900 / 10000;
        assert_eq!(min_out, expected_min);
        assert!(!tx_resp.transaction_data_bcs.is_empty());

        let swap_limits: Vec<u128> = tx_resp
            .debug_transaction
            .commands
            .iter()
            .filter_map(|c| match c {
                PtbCommand::MoveCall {
                    function,
                    arguments,
                    ..
                } if function == "swap" => match &arguments[7] {
                    PtbArgument::Pure(s) => Some(s.parse().unwrap()),
                    PtbArgument::U128(v) => Some(*v),
                    _ => None,
                },
                PtbCommand::MoveCall {
                    function,
                    arguments,
                    ..
                } if function == "flash_swap" => match &arguments[5] {
                    PtbArgument::Pure(s) => Some(s.parse().unwrap()),
                    PtbArgument::U128(v) => Some(*v),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert!(!swap_limits.is_empty());
        let static_min = crate::dex_swap::MIN_SQRT_PRICE.parse::<u128>().unwrap();
        assert!(swap_limits.iter().any(|limit| *limit != static_min));
    }
}
