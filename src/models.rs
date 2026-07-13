use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Token {
    pub address: String,
    pub symbol: String,
    pub name: String,
    pub decimals: u8,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QuoteRequest {
    pub from_token: String,
    pub to_token: String,
    pub amount: String, // String to prevent precision loss for large uint256 values
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteStep {
    pub dex_name: String,
    pub pool_address: String,
    pub weight: u32, // percentage of swap going through this route (e.g. 100 for direct route)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuoteResponse {
    pub from_token: String,
    pub to_token: String,
    pub amount_in: String,
    pub amount_out: String,
    pub price_impact: f64,
    pub route: Vec<RouteStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PoolState {
    pub pool_id: String,
    pub dex_name: String,
    pub coin_type_a: String,
    pub coin_type_b: String,
    pub sqrt_price: u128,
    pub liquidity: u128,
    pub fee_rate: u64,
    pub is_paused: bool,
}

impl PoolState {
    /// Calculates the price of Coin A in terms of Coin B.
    /// Formula: Price = (sqrt_price / 2^factor_bits)^2 * 10^(decimals_a - decimals_b)
    /// Common factor_bits: 64 for Cetus/Momentum, 96 for Turbos.
    pub fn calculate_price(&self, decimals_a: u8, decimals_b: u8, factor_bits: u32) -> f64 {
        let sqrt_price_f = self.sqrt_price as f64;
        let base = 2.0_f64.powi(factor_bits as i32);
        let price_internal = (sqrt_price_f / base).powi(2);
        let decimal_diff = decimals_a as i32 - decimals_b as i32;
        price_internal * 10.0_f64.powi(decimal_diff)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TickInfo {
    pub tick_index: i32,
    pub liquidity_net: i128,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PoolTickData {
    pub pool_id: String,
    pub current_tick_index: i32,
    pub tick_spacing: u32,
    /// Sorted ascending by tick_index
    pub ticks: Vec<TickInfo>,
    /// Pool-level fee growth for coin A (normalized; Momentum `x` maps here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee_growth_global_a: Option<u128>,
    /// Pool-level fee growth for coin B (normalized; Momentum `y` maps here).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fee_growth_global_b: Option<u128>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "fields")]
pub enum PtbCommand {
    SplitCoins {
        coin: PtbArgument,
        amounts: Vec<PtbArgument>,
    },
    MoveCall {
        package: String,
        module: String,
        function: String,
        type_arguments: Vec<String>,
        arguments: Vec<PtbArgument>,
    },
    TransferObjects {
        objects: Vec<PtbArgument>,
        address: PtbArgument,
    },
    MergeCoins {
        destination: PtbArgument,
        sources: Vec<PtbArgument>,
    },
    /// `MakeMoveVec` — used by Turbos `swap_router` which takes `vector<Coin<T>>`.
    MakeMoveVec {
        /// Element type tag (e.g. `0x2::coin::Coin<0x2::sui::SUI>`), if known.
        type_tag: Option<String>,
        elements: Vec<PtbArgument>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "value")]
pub enum PtbArgument {
    GasCoin,
    InputCoin,
    Result(u16),
    NestedResult(u16, u16),
    /// Legacy stringified pure value — prefer typed variants for canonical BCS.
    Pure(String),
    /// On-chain object id to resolve into ImmOrOwned / Shared input.
    Object(String),
    Bool(bool),
    U64(u64),
    U128(u128),
    Address(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtbTransaction {
    pub sender: String,
    pub commands: Vec<PtbCommand>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildTxRequest {
    pub from_token: String,
    pub to_token: String,
    pub amount: String,
    pub user_address: String,
    pub slippage_tolerance: f64,
    pub coin_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectRefSummary {
    pub object_id: String,
    pub version: u64,
    pub digest: String,
    pub owner_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildTxResponse {
    /// Base64-encoded BCS of canonical Sui `Transaction` (TransactionData V1).
    pub transaction_data_bcs: String,
    /// Base58 transaction digest for the unsigned transaction.
    pub transaction_digest: String,
    pub gas_budget: u64,
    pub gas_price: u64,
    pub object_refs: Vec<ObjectRefSummary>,
    /// Symbolic PTB plan for debugging — not wallet-signable.
    pub debug_transaction: PtbTransaction,
    pub estimated_amount_out: String,
    pub min_amount_out: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_price_cetus_style() {
        // Test Cetus-style pool state with 64 factor bits
        // Let's assume a 1:1 price pool for tokens with the same decimals.
        // sqrt(1) * 2^64 = 2^64 = 18446744073709551616
        let pool = PoolState {
            pool_id: "0x1".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "0xcoinA".to_string(),
            coin_type_b: "0xcoinB".to_string(),
            sqrt_price: 18446744073709551616, // 2^64
            liquidity: 1000,
            fee_rate: 3000,
            is_paused: false,
        };

        let price = pool.calculate_price(9, 9, 64);
        assert!((price - 1.0).abs() < 1e-9);

        // Test with decimal difference: Coin A has 18 decimals, Coin B has 6 decimals.
        // If human price is 3000 Coin B per Coin A.
        // P_internal = 3000 / 10^(18 - 6) = 3000 / 10^12 = 3.0e-9
        // sqrt_price_internal = sqrt(3.0e-9) = 5.477225575051661e-5
        // sqrt_price_scaled = sqrt_price_internal * 2^64 = 5.477225575051661e-5 * 1.8446744073709552e19 = 1010370000000000
        let pool_weth_usdc = PoolState {
            pool_id: "0x2".to_string(),
            dex_name: "Cetus".to_string(),
            coin_type_a: "WETH".to_string(),
            coin_type_b: "USDC".to_string(),
            sqrt_price: 1010370000000000,
            liquidity: 1000000,
            fee_rate: 3000,
            is_paused: false,
        };
        let computed_price = pool_weth_usdc.calculate_price(18, 6, 64);
        // Computed price should be around 3000.0 (allow small rounding difference)
        assert!((computed_price - 2999.98).abs() < 0.1);
    }

    #[test]
    fn test_calculate_price_turbos_style() {
        // Test Turbos-style pool state with 96 factor bits
        // Let's assume a 1:1 price pool for tokens with the same decimals.
        // sqrt(1) * 2^96 = 2^96 = 79228162514264337593543950336
        let pool = PoolState {
            pool_id: "0x3".to_string(),
            dex_name: "Turbos".to_string(),
            coin_type_a: "0xcoinA".to_string(),
            coin_type_b: "0xcoinB".to_string(),
            sqrt_price: 79228162514264337593543950336, // 2^96
            liquidity: 1000,
            fee_rate: 3000,
            is_paused: false,
        };

        let price = pool.calculate_price(9, 9, 96);
        assert!((price - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_pool_tick_data_legacy_json_without_fee_growth() {
        let legacy =
            r#"{"pool_id":"0x_pool","current_tick_index":10,"tick_spacing":60,"ticks":[]}"#;
        let parsed: PoolTickData = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.fee_growth_global_a, None);
        assert_eq!(parsed.fee_growth_global_b, None);
    }

    #[test]
    fn test_pool_tick_data_serde_roundtrip() {
        let data = PoolTickData {
            pool_id: "0x_pool".to_string(),
            current_tick_index: 100,
            tick_spacing: 60,
            ticks: vec![
                TickInfo {
                    tick_index: -60,
                    liquidity_net: 1_000_000,
                },
                TickInfo {
                    tick_index: 60,
                    liquidity_net: -500_000,
                },
            ],
            fee_growth_global_a: Some(100),
            fee_growth_global_b: Some(200),
        };
        let json = serde_json::to_string(&data).unwrap();
        let parsed: PoolTickData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, data);
    }
}
