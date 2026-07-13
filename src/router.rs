use crate::models::{PoolState, PoolTickData};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub pool_id: String,
    pub dex_name: String,
    pub fee_rate: u64,
}

#[derive(Debug, Clone, Default)]
pub struct TokenGraph {
    pub adjacency_list: HashMap<String, Vec<(String, Edge)>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwapHop {
    pub pool_id: String,
    pub dex_name: String,
    pub input_token: String,
    pub output_token: String,
    pub fee_rate: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutePath {
    pub hops: Vec<SwapHop>,
}

#[derive(Clone, Eq, PartialEq)]
struct DijkstraNode {
    token: String,
    cost: u64,
}

impl Ord for DijkstraNode {
    fn cmp(&self, other: &Self) -> Ordering {
        other.cost.cmp(&self.cost) // Min-heap behavior
    }
}

impl PartialOrd for DijkstraNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl TokenGraph {
    pub fn new() -> Self {
        Self {
            adjacency_list: HashMap::new(),
        }
    }

    /// Rebuilds the graph adjacency list from a slice of PoolState records
    pub fn build_from_pools(&mut self, pools: &[PoolState]) {
        self.adjacency_list.clear();
        for pool in pools {
            if pool.is_paused {
                continue;
            }

            // Edge from coin_type_a to coin_type_b
            let edge_ab = Edge {
                pool_id: pool.pool_id.clone(),
                dex_name: pool.dex_name.clone(),
                fee_rate: pool.fee_rate,
            };
            self.adjacency_list
                .entry(pool.coin_type_a.clone())
                .or_default()
                .push((pool.coin_type_b.clone(), edge_ab));

            // Edge from coin_type_b to coin_type_a
            let edge_ba = Edge {
                pool_id: pool.pool_id.clone(),
                dex_name: pool.dex_name.clone(),
                fee_rate: pool.fee_rate,
            };
            self.adjacency_list
                .entry(pool.coin_type_b.clone())
                .or_default()
                .push((pool.coin_type_a.clone(), edge_ba));
        }
    }

    /// Finds all simple paths from source to destination up to max_hops length
    pub fn find_all_paths_dfs(&self, from: &str, to: &str, max_hops: usize) -> Vec<RoutePath> {
        let mut paths = Vec::new();
        let mut current_path = Vec::new();
        let mut visited = HashSet::new();

        self.dfs_helper(
            from,
            to,
            max_hops,
            &mut visited,
            &mut current_path,
            &mut paths,
        );
        paths
    }

    fn dfs_helper(
        &self,
        current: &str,
        target: &str,
        max_hops: usize,
        visited: &mut HashSet<String>,
        current_path: &mut Vec<SwapHop>,
        paths: &mut Vec<RoutePath>,
    ) {
        if current == target {
            if !current_path.is_empty() {
                paths.push(RoutePath {
                    hops: current_path.clone(),
                });
            }
            return;
        }

        if current_path.len() >= max_hops {
            return;
        }

        visited.insert(current.to_string());

        if let Some(edges) = self.adjacency_list.get(current) {
            for (next_token, edge) in edges {
                if !visited.contains(next_token) {
                    let hop = SwapHop {
                        pool_id: edge.pool_id.clone(),
                        dex_name: edge.dex_name.clone(),
                        input_token: current.to_string(),
                        output_token: next_token.clone(),
                        fee_rate: edge.fee_rate,
                    };
                    current_path.push(hop);
                    self.dfs_helper(next_token, target, max_hops, visited, current_path, paths);
                    current_path.pop();
                }
            }
        }

        visited.remove(current);
    }

    /// Finds the shortest single path from source to target using Dijkstra's algorithm (minimizes hop count + fee cost)
    pub fn find_shortest_path_dijkstra(&self, from: &str, to: &str) -> Option<RoutePath> {
        let mut dists: HashMap<String, u64> = HashMap::new();
        let mut parent: HashMap<String, (String, Edge)> = HashMap::new();
        let mut heap = BinaryHeap::new();

        dists.insert(from.to_string(), 0u64);
        heap.push(DijkstraNode {
            token: from.to_string(),
            cost: 0,
        });

        while let Some(DijkstraNode { token, cost }) = heap.pop() {
            if token == to {
                // Reconstruct route path
                let mut hops = Vec::new();
                let mut curr = to.to_string();
                while let Some((prev, edge)) = parent.get(&curr) {
                    hops.push(SwapHop {
                        pool_id: edge.pool_id.clone(),
                        dex_name: edge.dex_name.clone(),
                        input_token: prev.clone(),
                        output_token: curr.clone(),
                        fee_rate: edge.fee_rate,
                    });
                    curr = prev.clone();
                }
                hops.reverse();
                return Some(RoutePath { hops });
            }

            if let Some(&curr_dist) = dists.get(&token)
                && cost > curr_dist
            {
                continue;
            }

            if let Some(edges) = self.adjacency_list.get(&token) {
                for (next_token, edge) in edges {
                    // Cost function: fee_rate + 100 (favors fewer hops & lower fees)
                    let weight = edge.fee_rate + 100;
                    let next_cost = cost + weight;

                    let should_update = match dists.get(next_token) {
                        Some(&d) => next_cost < d,
                        None => true,
                    };

                    if should_update {
                        dists.insert(next_token.clone(), next_cost);
                        parent.insert(next_token.clone(), (token.clone(), edge.clone()));
                        heap.push(DijkstraNode {
                            token: next_token.clone(),
                            cost: next_cost,
                        });
                    }
                }
            }
        }

        None
    }

    /// Finds the shortest single path and returns a boolean indicating if a negative cycle was detected (Bellman-Ford)
    pub fn find_shortest_path_bellman_ford(
        &self,
        from: &str,
        to: &str,
    ) -> Option<(RoutePath, bool)> {
        let mut dists = HashMap::new();
        let mut parent = HashMap::new();

        // Get all unique tokens (vertices)
        let mut tokens = HashSet::new();
        for (src, edges) in &self.adjacency_list {
            tokens.insert(src.clone());
            for (dest, _) in edges {
                tokens.insert(dest.clone());
            }
        }
        let num_vertices = tokens.len();

        dists.insert(from.to_string(), 0i64);

        // Relax edges V-1 times
        for _ in 0..num_vertices.saturating_sub(1) {
            let mut relaxed_any = false;
            for u in &tokens {
                if let Some(&u_dist) = dists.get(u)
                    && let Some(edges) = self.adjacency_list.get(u)
                {
                    for (v, edge) in edges {
                        let weight = edge.fee_rate as i64;
                        let new_dist = u_dist + weight;

                        let should_update = match dists.get(v) {
                            Some(&d) => new_dist < d,
                            None => true,
                        };

                        if should_update {
                            dists.insert(v.clone(), new_dist);
                            parent.insert(v.clone(), (u.clone(), edge.clone()));
                            relaxed_any = true;
                        }
                    }
                }
            }
            if !relaxed_any {
                break;
            }
        }

        // Check for negative cycles
        let mut has_negative_cycle = false;
        for u in &tokens {
            if let Some(&u_dist) = dists.get(u)
                && let Some(edges) = self.adjacency_list.get(u)
            {
                for (v, edge) in edges {
                    let weight = edge.fee_rate as i64;
                    if u_dist + weight < *dists.get(v).unwrap_or(&i64::MAX) {
                        has_negative_cycle = true;
                        break;
                    }
                }
            }
            if has_negative_cycle {
                break;
            }
        }

        if dists.contains_key(to) {
            let mut hops = Vec::new();
            let mut curr = to.to_string();
            let mut visited = HashSet::new();

            while let Some((prev, edge)) = parent.get(&curr) {
                if visited.contains(&curr) {
                    break; // prevent cycles in path reconstruction
                }
                visited.insert(curr.clone());
                hops.push(SwapHop {
                    pool_id: edge.pool_id.clone(),
                    dex_name: edge.dex_name.clone(),
                    input_token: prev.clone(),
                    output_token: curr.clone(),
                    fee_rate: edge.fee_rate,
                });
                curr = prev.clone();
            }
            hops.reverse();
            Some((RoutePath { hops }, has_negative_cycle))
        } else {
            None
        }
    }

    /// Find routes based on transaction volume threshold: Dijkstra for small amount, DFS for large amount.
    pub fn find_best_route(
        &self,
        from: &str,
        to: &str,
        amount: u128,
        config: &RouteConfig,
    ) -> Vec<RoutePath> {
        if amount <= config.small_amount_threshold {
            if let Some(path) = self.find_shortest_path_dijkstra(from, to) {
                vec![path]
            } else {
                Vec::new()
            }
        } else {
            self.find_all_paths_dfs(from, to, config.max_hops)
        }
    }
}

const MIN_SQRT_PRICE_X64: f64 = 4295048016.0;
const MAX_SQRT_PRICE_X64: f64 = 79226673515401279992447579055.0;

fn pool_factor_bits(pool: &PoolState) -> i32 {
    if pool.dex_name.to_lowercase() == "turbos" {
        96
    } else {
        64
    }
}

/// On-chain sqrt price scale bits for a pool (Turbos uses 96, others 64).
pub fn pool_sqrt_factor_bits(pool: &PoolState) -> i32 {
    pool_factor_bits(pool)
}

/// Detailed single-hop swap simulation including final internal sqrt price.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwapSimulationResult {
    pub amount_out: f64,
    pub marginal_price: f64,
    pub final_sqrt_price_internal: f64,
}

/// Unscaled internal sqrt price from tick index (price = 1.0001^tick).
pub fn tick_to_sqrt_price_internal(tick: i32) -> f64 {
    1.0001_f64.powf(tick as f64 / 2.0)
}

fn pick_next_tick(
    ticks: &[crate::models::TickInfo],
    current: i32,
    going_down: bool,
) -> Option<i32> {
    if going_down {
        ticks
            .iter()
            .filter(|t| t.tick_index < current)
            .map(|t| t.tick_index)
            .max()
    } else {
        ticks
            .iter()
            .filter(|t| t.tick_index > current)
            .map(|t| t.tick_index)
            .min()
    }
}

fn liquidity_net_of(ticks: &[crate::models::TickInfo], tick_index: i32) -> f64 {
    ticks
        .iter()
        .find(|t| t.tick_index == tick_index)
        .map(|t| t.liquidity_net as f64)
        .unwrap_or(0.0)
}

/// Single-hop within-tick CLMM swap (constant L, no tick crossing).
pub fn simulate_hop_within_tick(
    pool: &PoolState,
    hop: &SwapHop,
    amount_in: f64,
    decimals_map: &HashMap<String, u8>,
) -> (f64, f64) {
    let r = simulate_hop_within_tick_detailed(pool, hop, amount_in, decimals_map);
    (r.amount_out, r.marginal_price)
}

pub fn simulate_hop_within_tick_detailed(
    pool: &PoolState,
    hop: &SwapHop,
    amount_in: f64,
    _decimals_map: &HashMap<String, u8>,
) -> SwapSimulationResult {
    if amount_in <= 0.0 || pool.liquidity == 0 {
        let (_, spot) = simulate_hop_within_tick_spot(pool, hop);
        let factor_bits = pool_factor_bits(pool);
        let sqrt_price_internal = (pool.sqrt_price as f64) / 2.0f64.powi(factor_bits);
        return SwapSimulationResult {
            amount_out: 0.0,
            marginal_price: spot,
            final_sqrt_price_internal: sqrt_price_internal,
        };
    }

    let fee_factor = 1.0 - (pool.fee_rate as f64) / 1_000_000.0;
    let factor_bits = pool_factor_bits(pool);
    let mut sqrt_price_internal = (pool.sqrt_price as f64) / 2.0f64.powi(factor_bits);
    let l_f = pool.liquidity as f64;
    let dx = amount_in * fee_factor;

    let (next_amount, hop_marginal_price) = if hop.input_token == pool.coin_type_a {
        let x = l_f / sqrt_price_internal;
        let y = l_f * sqrt_price_internal;
        let dy = (y * dx) / (x + dx);
        sqrt_price_internal = l_f * sqrt_price_internal / (l_f + dx * sqrt_price_internal);
        let d_dy_d_dx = (y * x) / (x + dx).powi(2);
        (dy, d_dy_d_dx * fee_factor)
    } else {
        let x = l_f * sqrt_price_internal;
        let y = l_f / sqrt_price_internal;
        let dy = (y * dx) / (x + dx);
        sqrt_price_internal += dx / l_f;
        let d_dy_d_dx = (y * x) / (x + dx).powi(2);
        (dy, d_dy_d_dx * fee_factor)
    };

    SwapSimulationResult {
        amount_out: next_amount,
        marginal_price: hop_marginal_price,
        final_sqrt_price_internal: sqrt_price_internal,
    }
}

fn simulate_hop_within_tick_spot(pool: &PoolState, hop: &SwapHop) -> (f64, f64) {
    let fee_factor = 1.0 - (pool.fee_rate as f64) / 1_000_000.0;
    let factor_bits = pool_factor_bits(pool);
    let sqrt_price_internal = (pool.sqrt_price as f64) / 2.0f64.powi(factor_bits);
    let l_f = pool.liquidity as f64;

    if l_f == 0.0 {
        return (0.0, 0.0);
    }

    let marginal = if hop.input_token == pool.coin_type_a {
        let x = l_f / sqrt_price_internal;
        let y = l_f * sqrt_price_internal;
        (y * x) / (x * x) * fee_factor
    } else {
        let x = l_f * sqrt_price_internal;
        let y = l_f / sqrt_price_internal;
        (y * x) / (x * x) * fee_factor
    };
    (0.0, marginal)
}

/// V3-style swap with tick crossing when tick data is available.
pub fn simulate_swap_tick_aware(
    pool: &PoolState,
    td: &PoolTickData,
    hop: &SwapHop,
    amount_in: f64,
    decimals_map: &HashMap<String, u8>,
) -> (f64, f64) {
    let r = simulate_swap_tick_aware_detailed(pool, td, hop, amount_in, decimals_map);
    (r.amount_out, r.marginal_price)
}

pub fn simulate_swap_tick_aware_detailed(
    pool: &PoolState,
    td: &PoolTickData,
    hop: &SwapHop,
    amount_in: f64,
    decimals_map: &HashMap<String, u8>,
) -> SwapSimulationResult {
    if td.ticks.is_empty() {
        return simulate_hop_within_tick_detailed(pool, hop, amount_in, decimals_map);
    }

    if amount_in <= 0.0 {
        return simulate_hop_within_tick_detailed(pool, hop, 0.0, decimals_map);
    }

    let factor_bits = pool_factor_bits(pool);
    let scale = 2.0f64.powi(factor_bits);
    let fee_factor = 1.0 - (pool.fee_rate as f64) / 1_000_000.0;
    let mut remaining = amount_in * fee_factor;
    let mut sqrtp = pool.sqrt_price as f64 / scale;
    let mut l = pool.liquidity as f64;
    let mut tick_cur = td.current_tick_index;
    let going_down = hop.input_token == pool.coin_type_a;
    let mut out = 0.0;
    let mut last_dx = 0.0;

    let max_iter = td.ticks.len() + 2;
    for _ in 0..max_iter {
        if remaining <= 1e-12 {
            break;
        }

        let next_tick = pick_next_tick(&td.ticks, tick_cur, going_down);
        let sqrtp_target = match next_tick {
            Some(t) => tick_to_sqrt_price_internal(t),
            None => {
                if going_down {
                    MIN_SQRT_PRICE_X64 / scale
                } else {
                    MAX_SQRT_PRICE_X64 / scale
                }
            }
        };

        if going_down {
            if sqrtp <= sqrtp_target || l <= 0.0 {
                break;
            }
            let delta_needed = l * (sqrtp - sqrtp_target) / (sqrtp * sqrtp_target);
            if remaining <= delta_needed {
                let sqrtp_new = l * sqrtp / (l + remaining * sqrtp);
                out += l * (sqrtp - sqrtp_new);
                last_dx = remaining;
                sqrtp = sqrtp_new;
                remaining = 0.0;
            } else {
                out += l * (sqrtp - sqrtp_target);
                remaining -= delta_needed;
                last_dx = delta_needed;
                sqrtp = sqrtp_target;
                if let Some(t) = next_tick {
                    l -= liquidity_net_of(&td.ticks, t);
                    tick_cur = t;
                } else {
                    break;
                }
            }
        } else {
            if sqrtp >= sqrtp_target || l <= 0.0 {
                break;
            }
            let delta_needed = l * (sqrtp_target - sqrtp);
            if remaining <= delta_needed {
                let sqrtp_new = sqrtp + remaining / l;
                out += l * (sqrtp_new - sqrtp) / (sqrtp * sqrtp_new);
                last_dx = remaining;
                sqrtp = sqrtp_new;
                remaining = 0.0;
            } else {
                out += l * (sqrtp_target - sqrtp) / (sqrtp * sqrtp_target);
                remaining -= delta_needed;
                last_dx = delta_needed;
                sqrtp = sqrtp_target;
                if let Some(t) = next_tick {
                    l += liquidity_net_of(&td.ticks, t);
                    tick_cur = t;
                } else {
                    break;
                }
            }
        }
    }

    if l <= 0.0 || sqrtp <= 0.0 {
        return SwapSimulationResult {
            amount_out: out.max(0.0),
            marginal_price: 0.0,
            final_sqrt_price_internal: sqrtp.max(0.0),
        };
    }

    let x = l / sqrtp;
    let y = l * sqrtp;
    let dx_eff = last_dx.max(1e-18);
    let marginal = (y * x) / (x + dx_eff).powi(2) * fee_factor;
    SwapSimulationResult {
        amount_out: out.max(0.0),
        marginal_price: marginal,
        final_sqrt_price_internal: sqrtp,
    }
}

/// Route simulation with optional per-pool tick data (falls back to within-tick).
pub fn simulate_route_tick_aware(
    route: &RoutePath,
    amount_in: f64,
    pools_map: &HashMap<String, PoolState>,
    ticks_map: &HashMap<String, PoolTickData>,
    decimals_map: &HashMap<String, u8>,
) -> (f64, f64) {
    let mut current_amount = amount_in;
    let mut current_marginal_price = 1.0;

    for hop in &route.hops {
        let pool = match pools_map.get(&hop.pool_id) {
            Some(p) => p,
            None => return (0.0, 0.0),
        };

        let (next_amount, hop_marginal) = if let Some(td) = ticks_map.get(&hop.pool_id) {
            simulate_swap_tick_aware(pool, td, hop, current_amount, decimals_map)
        } else {
            simulate_hop_within_tick(pool, hop, current_amount, decimals_map)
        };

        current_amount = next_amount;
        current_marginal_price *= hop_marginal;
    }

    (current_amount, current_marginal_price)
}

pub fn simulate_route_with_marginal_price(
    route: &RoutePath,
    amount_in: f64,
    pools_map: &HashMap<String, PoolState>,
    decimals_map: &HashMap<String, u8>,
) -> (f64, f64) {
    simulate_route_tick_aware(route, amount_in, pools_map, &HashMap::new(), decimals_map)
}

fn solve_allocation_for_target_marginal(
    route: &RoutePath,
    target_marginal: f64,
    max_amount: f64,
    pools_map: &HashMap<String, PoolState>,
    ticks_map: &HashMap<String, PoolTickData>,
    decimals_map: &HashMap<String, u8>,
) -> f64 {
    let (_, spot_marginal) =
        simulate_route_tick_aware(route, 0.0, pools_map, ticks_map, decimals_map);
    if spot_marginal <= target_marginal {
        return 0.0;
    }

    let mut low = 0.0;
    let mut high = max_amount;
    for _ in 0..15 {
        let mid = (low + high) / 2.0;
        let (_, marginal) =
            simulate_route_tick_aware(route, mid, pools_map, ticks_map, decimals_map);
        if marginal > target_marginal {
            low = mid;
        } else {
            high = mid;
        }
    }
    (low + high) / 2.0
}

pub fn optimize_order_split_gross(
    routes: &[RoutePath],
    amount_in: u128,
    pools_map: &HashMap<String, PoolState>,
    decimals_map: &HashMap<String, u8>,
    ticks_map: &HashMap<String, PoolTickData>,
) -> Vec<(RoutePath, u128, u128)> {
    if routes.is_empty() || amount_in == 0 {
        return Vec::new();
    }

    if routes.len() == 1 {
        let (out, _) = simulate_route_tick_aware(
            &routes[0],
            amount_in as f64,
            pools_map,
            ticks_map,
            decimals_map,
        );
        return vec![(routes[0].clone(), amount_in, out as u128)];
    }

    let total_amount_f = amount_in as f64;

    let mut max_spot = 0.0;
    for r in routes {
        let (_, spot) = simulate_route_tick_aware(r, 0.0, pools_map, ticks_map, decimals_map);
        if spot > max_spot {
            max_spot = spot;
        }
    }

    let mut low = 0.0;
    let mut high = max_spot;
    let mut best_allocations = vec![0.0; routes.len()];

    for _ in 0..20 {
        let mid = (low + high) / 2.0;
        let mut sum = 0.0;
        let mut allocs = vec![0.0; routes.len()];

        for (i, r) in routes.iter().enumerate() {
            let alloc = solve_allocation_for_target_marginal(
                r,
                mid,
                total_amount_f,
                pools_map,
                ticks_map,
                decimals_map,
            );
            allocs[i] = alloc;
            sum += alloc;
        }

        if sum > total_amount_f {
            low = mid;
        } else {
            high = mid;
            best_allocations = allocs;
        }
    }

    let sum_alloc: f64 = best_allocations.iter().sum();
    let mut result = Vec::new();

    if sum_alloc > 0.0 {
        let scale = total_amount_f / sum_alloc;
        let mut allocated_sum_u128 = 0;

        for (i, r) in routes.iter().enumerate() {
            let alloc_f = best_allocations[i] * scale;
            let alloc_u = alloc_f as u128;

            if alloc_u > 0 {
                let (out, _) =
                    simulate_route_tick_aware(r, alloc_f, pools_map, ticks_map, decimals_map);
                result.push((r.clone(), alloc_u, out as u128));
                allocated_sum_u128 += alloc_u;
            }
        }

        if allocated_sum_u128 < amount_in && !result.is_empty() {
            let diff = amount_in - allocated_sum_u128;
            if let Some(max_item) = result.iter_mut().max_by_key(|item| item.1) {
                max_item.1 += diff;
                let (out, _) = simulate_route_tick_aware(
                    &max_item.0,
                    max_item.1 as f64,
                    pools_map,
                    ticks_map,
                    decimals_map,
                );
                max_item.2 = out as u128;
            }
        }
    } else {
        let (out, _) = simulate_route_tick_aware(
            &routes[0],
            total_amount_f,
            pools_map,
            ticks_map,
            decimals_map,
        );
        result.push((routes[0].clone(), amount_in, out as u128));
    }

    result
}

fn get_sui_price_in_target_token(
    target_token: &str,
    pools_map: &HashMap<String, PoolState>,
    decimals_map: &HashMap<String, u8>,
) -> f64 {
    let target_norm = target_token.to_lowercase();
    if target_norm == "sui" || target_norm.contains("0x2::sui::sui") {
        return 1.0;
    }

    for pool in pools_map.values() {
        let coin_a = pool.coin_type_a.to_lowercase();
        let coin_b = pool.coin_type_b.to_lowercase();
        let is_a_sui = coin_a == "sui" || coin_a.contains("0x2::sui::sui");
        let is_b_sui = coin_b == "sui" || coin_b.contains("0x2::sui::sui");

        if is_a_sui && coin_b == target_norm {
            let dec_a = *decimals_map.get(&pool.coin_type_a).unwrap_or(&9);
            let dec_b = *decimals_map.get(&pool.coin_type_b).unwrap_or(&9);
            let factor_bits = if pool.dex_name.to_lowercase() == "turbos" {
                96
            } else {
                64
            };
            return pool.calculate_price(dec_a, dec_b, factor_bits);
        } else if coin_a == target_norm && is_b_sui {
            let dec_a = *decimals_map.get(&pool.coin_type_a).unwrap_or(&9);
            let dec_b = *decimals_map.get(&pool.coin_type_b).unwrap_or(&9);
            let factor_bits = if pool.dex_name.to_lowercase() == "turbos" {
                96
            } else {
                64
            };
            let price = pool.calculate_price(dec_a, dec_b, factor_bits);
            if price > 0.0 {
                return 1.0 / price;
            }
        }
    }

    2.5
}

pub fn calculate_route_gas_cost_in_target_token(
    route: &RoutePath,
    target_token: &str,
    reference_gas_price: u64,
    pools_map: &HashMap<String, PoolState>,
    decimals_map: &HashMap<String, u8>,
) -> f64 {
    let gas_units_per_hop = 5000.0;
    let total_gas_units = (route.hops.len() as f64) * gas_units_per_hop;
    let total_gas_mist = total_gas_units * (reference_gas_price as f64);
    let total_gas_sui = total_gas_mist / 1_000_000_000.0;

    let target_decimals = *decimals_map.get(target_token).unwrap_or(&9);
    let sui_price = get_sui_price_in_target_token(target_token, pools_map, decimals_map);

    let gas_cost_target_human = total_gas_sui * sui_price;
    gas_cost_target_human * 10.0f64.powi(target_decimals as i32)
}

pub fn optimize_order_split(
    routes: &[RoutePath],
    amount_in: u128,
    to_token: &str,
    reference_gas_price: u64,
    pools_map: &HashMap<String, PoolState>,
    decimals_map: &HashMap<String, u8>,
    ticks_map: &HashMap<String, PoolTickData>,
) -> Vec<(RoutePath, u128, u128)> {
    if routes.is_empty() || amount_in == 0 {
        return Vec::new();
    }

    let pruned_routes: Vec<RoutePath> = routes
        .iter()
        .filter(|r| r.hops.len() <= 3)
        .cloned()
        .collect();

    if pruned_routes.is_empty() {
        return Vec::new();
    }

    let mut active_routes = pruned_routes.clone();
    let mut best_splits = Vec::new();
    let mut best_net_out = -1.0;

    loop {
        let splits = optimize_order_split_gross(
            &active_routes,
            amount_in,
            pools_map,
            decimals_map,
            ticks_map,
        );
        if splits.is_empty() {
            break;
        }

        let mut current_net_out = 0.0;
        let mut route_to_remove = None;
        let mut min_contribution = f64::MAX;
        let mut min_contribution_idx = None;

        for (i, (r, _alloc_in, alloc_out)) in splits.iter().enumerate() {
            let gas_cost = calculate_route_gas_cost_in_target_token(
                r,
                to_token,
                reference_gas_price,
                pools_map,
                decimals_map,
            );
            let net_out = (*alloc_out as f64) - gas_cost;

            if net_out <= 0.0 {
                route_to_remove = Some(i);
                break;
            }

            current_net_out += net_out;

            let contribution = net_out;
            if contribution < min_contribution {
                min_contribution = contribution;
                min_contribution_idx = Some(i);
            }
        }

        if let Some(idx) = route_to_remove {
            // F-07 fix: match by pool_id, not by splits vector index
            let pool_id_to_remove = splits[idx].0.hops[0].pool_id.clone();
            if let Some(pos) = active_routes
                .iter()
                .position(|r| r.hops[0].pool_id == pool_id_to_remove)
            {
                active_routes.remove(pos);
            }
            if active_routes.is_empty() {
                break;
            }
            continue;
        }

        if current_net_out > best_net_out {
            best_net_out = current_net_out;
            best_splits = splits.clone();

            if let Some(idx) = min_contribution_idx
                && active_routes.len() > 1
            {
                // F-07 fix: match by pool_id, not by splits vector index
                let pool_id_to_remove = splits[idx].0.hops[0].pool_id.clone();
                if let Some(pos) = active_routes
                    .iter()
                    .position(|r| r.hops[0].pool_id == pool_id_to_remove)
                {
                    active_routes.remove(pos);
                }
                continue;
            }
        }

        break;
    }

    if best_splits.is_empty() {
        let mut best_route = &pruned_routes[0];
        let mut max_out = 0;
        for r in &pruned_routes {
            let (out, _) =
                simulate_route_tick_aware(r, amount_in as f64, pools_map, ticks_map, decimals_map);
            if out as u128 > max_out {
                max_out = out as u128;
                best_route = r;
            }
        }
        return vec![(best_route.clone(), amount_in, max_out)];
    }

    best_splits
}

#[derive(Debug, Clone)]
pub struct RouteConfig {
    pub small_amount_threshold: u128,
    pub max_hops: usize,
}

impl Default for RouteConfig {
    fn default() -> Self {
        Self {
            small_amount_threshold: 100_000_000,
            max_hops: 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_pools() -> Vec<PoolState> {
        vec![
            PoolState {
                pool_id: "pool_sui_usdc_cetus".to_string(),
                dex_name: "Cetus".to_string(),
                coin_type_a: "SUI".to_string(),
                coin_type_b: "USDC".to_string(),
                sqrt_price: 100,
                liquidity: 1000,
                fee_rate: 3000,
                is_paused: false,
            },
            PoolState {
                pool_id: "pool_sui_usdc_turbos".to_string(),
                dex_name: "Turbos".to_string(),
                coin_type_a: "SUI".to_string(),
                coin_type_b: "USDC".to_string(),
                sqrt_price: 100,
                liquidity: 1500,
                fee_rate: 2500,
                is_paused: false,
            },
            PoolState {
                pool_id: "pool_usdc_usdt_momentum".to_string(),
                dex_name: "Momentum".to_string(),
                coin_type_a: "USDC".to_string(),
                coin_type_b: "USDT".to_string(),
                sqrt_price: 100,
                liquidity: 800,
                fee_rate: 1000,
                is_paused: false,
            },
            PoolState {
                pool_id: "pool_sui_usdt_magma".to_string(),
                dex_name: "Magma".to_string(),
                coin_type_a: "SUI".to_string(),
                coin_type_b: "USDT".to_string(),
                sqrt_price: 100,
                liquidity: 100,
                fee_rate: 5000,
                is_paused: false,
            },
        ]
    }

    #[test]
    fn test_dfs_find_all_paths() {
        let mut graph = TokenGraph::new();
        graph.build_from_pools(&sample_pools());

        // Find all paths from SUI to USDT with max 2 hops
        // Candidates:
        // 1. SUI -> USDT (1 hop, Magma)
        // 2. SUI -> USDC -> USDT (2 hops, Cetus + Momentum)
        // 3. SUI -> USDC -> USDT (2 hops, Turbos + Momentum)
        let paths = graph.find_all_paths_dfs("SUI", "USDT", 2);
        assert_eq!(paths.len(), 3);

        // Max 1 hop: should only return 1 path
        let paths_1 = graph.find_all_paths_dfs("SUI", "USDT", 1);
        assert_eq!(paths_1.len(), 1);
        assert_eq!(paths_1[0].hops[0].pool_id, "pool_sui_usdt_magma");
    }

    #[test]
    fn test_dijkstra_shortest_path() {
        let mut graph = TokenGraph::new();
        graph.build_from_pools(&sample_pools());

        // Shortest path from SUI to USDT:
        // SUI -> USDC -> USDT (turbos fee=2500 + momentum fee=1000 + 200 hop penalty = 3700)
        // vs SUI -> USDT (magma fee=5000 + 100 hop penalty = 5100)
        // Thus, the two-hop route should be chosen because the total cost is lower!
        let shortest = graph.find_shortest_path_dijkstra("SUI", "USDT").unwrap();
        assert_eq!(shortest.hops.len(), 2);
        assert_eq!(shortest.hops[0].pool_id, "pool_sui_usdc_turbos");
        assert_eq!(shortest.hops[1].pool_id, "pool_usdc_usdt_momentum");
    }

    #[test]
    fn test_bellman_ford_path_and_cycle() {
        let mut graph = TokenGraph::new();
        graph.build_from_pools(&sample_pools());

        let res = graph
            .find_shortest_path_bellman_ford("SUI", "USDT")
            .unwrap();
        let path = res.0;
        let negative_cycle = res.1;

        assert_eq!(path.hops.len(), 2);
        assert_eq!(path.hops[0].pool_id, "pool_sui_usdc_turbos");
        assert!(!negative_cycle);
    }

    #[test]
    fn test_threshold_routing() {
        let mut graph = TokenGraph::new();
        graph.build_from_pools(&sample_pools());

        let config = RouteConfig {
            small_amount_threshold: 1000,
            max_hops: 2,
        };

        // Small amount (Dijkstra)
        let small_routes = graph.find_best_route("SUI", "USDT", 500, &config);
        assert_eq!(small_routes.len(), 1);
        assert_eq!(small_routes[0].hops.len(), 2);
        assert_eq!(small_routes[0].hops[0].pool_id, "pool_sui_usdc_turbos");

        // Large amount (DFS)
        let large_routes = graph.find_best_route("SUI", "USDT", 2000, &config);
        assert_eq!(large_routes.len(), 3);
    }

    #[test]
    fn test_optimize_order_split_two_routes() {
        let pool_cetus = PoolState {
            pool_id: "pool_cetus".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "SUI".to_string(),
            coin_type_b: "USDC".to_string(),
            sqrt_price: 1 << 64, // Price 1.0
            liquidity: 1000000000,
            fee_rate: 3000,
            is_paused: false,
        };
        let pool_turbos = PoolState {
            pool_id: "pool_turbos".to_string(),
            dex_name: "Turbos".to_string(),
            coin_type_a: "SUI".to_string(),
            coin_type_b: "USDC".to_string(),
            sqrt_price: 1 << 96, // Price 1.0
            liquidity: 2000000000,
            fee_rate: 3000,
            is_paused: false,
        };

        let route_cetus = RoutePath {
            hops: vec![SwapHop {
                pool_id: "pool_cetus".to_string(),
                dex_name: "Cetus".to_string(),
                input_token: "SUI".to_string(),
                output_token: "USDC".to_string(),
                fee_rate: 3000,
            }],
        };
        let route_turbos = RoutePath {
            hops: vec![SwapHop {
                pool_id: "pool_turbos".to_string(),
                dex_name: "Turbos".to_string(),
                input_token: "SUI".to_string(),
                output_token: "USDC".to_string(),
                fee_rate: 3000,
            }],
        };

        let mut pools_map = HashMap::new();
        pools_map.insert("pool_cetus".to_string(), pool_cetus);
        pools_map.insert("pool_turbos".to_string(), pool_turbos);

        let mut decimals_map = HashMap::new();
        decimals_map.insert("SUI".to_string(), 9);
        decimals_map.insert("USDC".to_string(), 9);

        let routes = vec![route_cetus, route_turbos];
        let splits = optimize_order_split(
            &routes,
            200000000,
            "USDC",
            1000,
            &pools_map,
            &decimals_map,
            &HashMap::new(),
        );

        assert_eq!(splits.len(), 2);
        let turbos_alloc = splits
            .iter()
            .find(|s| s.0.hops[0].pool_id == "pool_turbos")
            .unwrap()
            .1;
        let cetus_alloc = splits
            .iter()
            .find(|s| s.0.hops[0].pool_id == "pool_cetus")
            .unwrap()
            .1;

        assert!(turbos_alloc > cetus_alloc);
        assert_eq!(turbos_alloc + cetus_alloc, 200000000);
    }

    #[test]
    fn test_optimize_order_split_gas_pruning() {
        let pool_cetus = PoolState {
            pool_id: "pool_cetus".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "SUI".to_string(),
            coin_type_b: "USDC".to_string(),
            sqrt_price: 1 << 64,
            liquidity: 1000000,
            fee_rate: 3000,
            is_paused: false,
        };
        let pool_turbos = PoolState {
            pool_id: "pool_turbos".to_string(),
            dex_name: "Turbos".to_string(),
            coin_type_a: "SUI".to_string(),
            coin_type_b: "USDC".to_string(),
            sqrt_price: 1 << 96,
            liquidity: 2000000,
            fee_rate: 3000,
            is_paused: false,
        };

        let route_cetus = RoutePath {
            hops: vec![SwapHop {
                pool_id: "pool_cetus".to_string(),
                dex_name: "Cetus".to_string(),
                input_token: "SUI".to_string(),
                output_token: "USDC".to_string(),
                fee_rate: 3000,
            }],
        };
        let route_turbos = RoutePath {
            hops: vec![SwapHop {
                pool_id: "pool_turbos".to_string(),
                dex_name: "Turbos".to_string(),
                input_token: "SUI".to_string(),
                output_token: "USDC".to_string(),
                fee_rate: 3000,
            }],
        };

        let mut pools_map = HashMap::new();
        pools_map.insert("pool_cetus".to_string(), pool_cetus);
        pools_map.insert("pool_turbos".to_string(), pool_turbos);

        let mut decimals_map = HashMap::new();
        decimals_map.insert("SUI".to_string(), 9);
        decimals_map.insert("USDC".to_string(), 9);

        let routes = vec![route_cetus, route_turbos];
        let splits = optimize_order_split(
            &routes,
            10000,
            "USDC",
            1000,
            &pools_map,
            &decimals_map,
            &HashMap::new(),
        );

        assert_eq!(splits.len(), 1);
        assert_eq!(splits[0].0.hops[0].pool_id, "pool_turbos");
        assert_eq!(splits[0].1, 10000);
    }

    #[test]
    fn test_tick_aware_fallback_equiv_empty_ticks() {
        let pool = PoolState {
            pool_id: "pool_cetus".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "SUI".to_string(),
            coin_type_b: "USDC".to_string(),
            sqrt_price: 1 << 64,
            liquidity: 1_000_000_000,
            fee_rate: 3000,
            is_paused: false,
        };
        let hop = SwapHop {
            pool_id: "pool_cetus".to_string(),
            dex_name: "Cetus".to_string(),
            input_token: "SUI".to_string(),
            output_token: "USDC".to_string(),
            fee_rate: 3000,
        };
        let td = PoolTickData {
            pool_id: "pool_cetus".to_string(),
            current_tick_index: 0,
            tick_spacing: 60,
            ticks: vec![],
            ..Default::default()
        };
        let decimals_map = HashMap::new();
        let amount = 100_000.0;

        let (within_out, within_m) = simulate_hop_within_tick(&pool, &hop, amount, &decimals_map);
        let (tick_out, tick_m) = simulate_swap_tick_aware(&pool, &td, &hop, amount, &decimals_map);

        assert!((within_out - tick_out).abs() < 1e-9);
        assert!((within_m - tick_m).abs() < 1e-9);

        let detailed = simulate_swap_tick_aware_detailed(&pool, &td, &hop, amount, &decimals_map);
        assert!((detailed.amount_out - tick_out).abs() < 1e-9);
        assert!((detailed.marginal_price - tick_m).abs() < 1e-9);
        assert!(detailed.final_sqrt_price_internal > 0.0);
    }

    #[test]
    fn test_final_sqrt_price_monotonic_a2b() {
        let pool = PoolState {
            pool_id: "pool_mono".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "SUI".to_string(),
            coin_type_b: "USDC".to_string(),
            sqrt_price: 1 << 64,
            liquidity: 1_000_000_000,
            fee_rate: 3000,
            is_paused: false,
        };
        let hop = SwapHop {
            pool_id: "pool_mono".to_string(),
            dex_name: "Cetus".to_string(),
            input_token: "SUI".to_string(),
            output_token: "USDC".to_string(),
            fee_rate: 3000,
        };
        let decimals_map = HashMap::new();
        let factor_bits = pool_sqrt_factor_bits(&pool);
        let start_sqrt = (pool.sqrt_price as f64) / 2.0f64.powi(factor_bits);

        let small = simulate_hop_within_tick_detailed(&pool, &hop, 1_000.0, &decimals_map);
        let large = simulate_hop_within_tick_detailed(&pool, &hop, 100_000.0, &decimals_map);

        assert!(small.final_sqrt_price_internal <= start_sqrt);
        assert!(large.final_sqrt_price_internal <= small.final_sqrt_price_internal);
        assert!(large.amount_out > small.amount_out);
    }

    #[test]
    fn test_tick_aware_crossing_reduces_liquidity() {
        let pool = PoolState {
            pool_id: "pool_cross".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "SUI".to_string(),
            coin_type_b: "USDC".to_string(),
            sqrt_price: tick_to_sqrt_price_internal(0) as u128 * (1u128 << 64),
            liquidity: 1_000_000,
            fee_rate: 0,
            is_paused: false,
        };
        let hop = SwapHop {
            pool_id: "pool_cross".to_string(),
            dex_name: "Cetus".to_string(),
            input_token: "SUI".to_string(),
            output_token: "USDC".to_string(),
            fee_rate: 0,
        };
        let td_no_cross = PoolTickData {
            pool_id: "pool_cross".to_string(),
            current_tick_index: 0,
            tick_spacing: 10,
            ticks: vec![],
            ..Default::default()
        };
        let td_with_cross = PoolTickData {
            pool_id: "pool_cross".to_string(),
            current_tick_index: 0,
            tick_spacing: 10,
            ticks: vec![crate::models::TickInfo {
                tick_index: -10,
                liquidity_net: -500_000,
            }],
            ..Default::default()
        };
        let decimals_map = HashMap::new();
        let large_in = 50_000.0;

        let (small_out, _) =
            simulate_swap_tick_aware(&pool, &td_no_cross, &hop, 100.0, &decimals_map);
        let (large_no_cross, _) =
            simulate_swap_tick_aware(&pool, &td_no_cross, &hop, large_in, &decimals_map);
        let (large_with_cross, _) =
            simulate_swap_tick_aware(&pool, &td_with_cross, &hop, large_in, &decimals_map);

        assert!(small_out > 0.0);
        assert!(large_no_cross > small_out);
        assert!(large_with_cross > 0.0);
        assert!(large_with_cross != large_no_cross);
    }

    #[test]
    fn test_tick_to_sqrt_price_at_zero() {
        let sp = tick_to_sqrt_price_internal(0);
        assert!((sp - 1.0).abs() < 1e-9);
    }
}
