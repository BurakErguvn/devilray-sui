//! DEX-specific PTB hop builders for swap execution.
//!
//! Turbos: `swap_router::{swap_a_b,swap_b_a}` entry (MakeMoveVec + recipient transfer).
//! Cetus/Magma/Momentum: flash-swap + into_balance / from_balance for Coin ↔ Balance.

use crate::discovery::registry::{
    CETUS_GLOBAL_CONFIG, MAGMA_GLOBAL_CONFIG, MOMENTUM_VERSION_OBJECT, SUI_CLOCK,
    SUI_FRAMEWORK_PACKAGE, TURBOS_VERSIONED, swap_contract_for_dex_name,
};
use crate::models::{PoolState, PtbArgument, PtbCommand};
use crate::router::SwapHop;

pub const MIN_SQRT_PRICE: &str = "4295048016";
pub const MAX_SQRT_PRICE: &str = "79226673515401279992447579055";

// Re-exports for probes / older call sites.
pub const CETUS_PACKAGE: &str = crate::discovery::registry::CETUS_SWAP_PACKAGE;
pub const CETUS_CONFIG: &str = CETUS_GLOBAL_CONFIG;
pub const TURBOS_PACKAGE: &str = crate::discovery::registry::TURBOS_SWAP_PACKAGE;
pub const MAGMA_PACKAGE: &str = crate::discovery::registry::MAGMA_SWAP_PACKAGE;
pub const MOMENTUM_PACKAGE: &str = crate::discovery::registry::MOMENTUM_DISCOVERY_PACKAGE;
pub const MOMENTUM_VERSION: &str = MOMENTUM_VERSION_OBJECT;
pub const TURBOS_VERSION: &str = TURBOS_VERSIONED;

/// Far-future deadline (ms) for Turbos `swap_router` entry calls.
const TURBOS_DEADLINE_MS: u64 = 9_999_999_999_999;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DexSwapKind {
    Cetus,
    Turbos,
    Magma,
    Momentum,
    Unsupported,
}

pub fn dex_kind(dex_name: &str) -> DexSwapKind {
    let lower = dex_name.to_lowercase();
    if lower.contains("cetus") {
        DexSwapKind::Cetus
    } else if lower.contains("turbos") {
        DexSwapKind::Turbos
    } else if lower.contains("magma") {
        DexSwapKind::Magma
    } else if lower == "momentum" || lower.contains("momentum") {
        DexSwapKind::Momentum
    } else {
        DexSwapKind::Unsupported
    }
}

fn package_for(dex: DexSwapKind) -> &'static str {
    let name = match dex {
        DexSwapKind::Cetus => "Cetus",
        DexSwapKind::Turbos => "Turbos",
        DexSwapKind::Magma => "Magma",
        DexSwapKind::Momentum => "Momentum",
        DexSwapKind::Unsupported => panic!("unsupported DEX"),
    };
    swap_contract_for_dex_name(name)
        .expect("swap contract registered")
        .package_id
}

/// Builds PTB commands for a single routing hop.
///
/// Returns `None` as the output coin when the hop already transfers to `recipient`
/// (Turbos `swap_router` entry). Turbos may only be used as a terminal hop.
#[allow(clippy::too_many_arguments)]
pub fn build_hop_commands(
    pool: &PoolState,
    hop: &SwapHop,
    current_coin: PtbArgument,
    expected_amount: u128,
    hop_min_out: u128,
    sqrt_price_limit_raw: u128,
    start_cmd_idx: u16,
    recipient: &str,
    is_final_hop: bool,
) -> Result<(Vec<PtbCommand>, Option<PtbArgument>), String> {
    let is_a_to_b = hop.input_token == pool.coin_type_a;
    match dex_kind(&pool.dex_name) {
        DexSwapKind::Cetus => {
            let (cmds, out) = build_cetus_flash_hop(
                pool,
                current_coin,
                is_a_to_b,
                expected_amount,
                sqrt_price_limit_raw,
                start_cmd_idx,
            );
            Ok((cmds, Some(out)))
        }
        DexSwapKind::Turbos => build_turbos_hop(
            pool,
            current_coin,
            is_a_to_b,
            expected_amount,
            hop_min_out,
            sqrt_price_limit_raw,
            start_cmd_idx,
            recipient,
            is_final_hop,
        ),
        DexSwapKind::Magma => {
            let (cmds, out) = build_magma_flash_hop(
                pool,
                current_coin,
                is_a_to_b,
                expected_amount,
                sqrt_price_limit_raw,
                start_cmd_idx,
            );
            Ok((cmds, Some(out)))
        }
        DexSwapKind::Momentum => {
            let (cmds, out) = build_momentum_flash_hop(
                pool,
                current_coin,
                is_a_to_b,
                expected_amount,
                sqrt_price_limit_raw,
                start_cmd_idx,
            );
            Ok((cmds, Some(out)))
        }
        DexSwapKind::Unsupported => Err(format!("unsupported DEX for swap: {}", pool.dex_name)),
    }
}

fn amount_u64(amount: u128) -> u64 {
    u64::try_from(amount).expect("swap amount must fit in u64")
}

/// Cetus mainnet CLMM uses `flash_swap` / `repay_flash_swap` (no single `pool::swap`).
fn build_cetus_flash_hop(
    pool: &PoolState,
    current_coin: PtbArgument,
    is_a_to_b: bool,
    expected_amount: u128,
    sqrt_price_limit: u128,
    start_cmd_idx: u16,
) -> (Vec<PtbCommand>, PtbArgument) {
    build_config_pool_flash_hop(
        package_for(DexSwapKind::Cetus),
        CETUS_GLOBAL_CONFIG,
        pool,
        current_coin,
        is_a_to_b,
        expected_amount,
        sqrt_price_limit,
        start_cmd_idx,
    )
}

/// Turbos fee type modules shipped in the CLMM package (`fee{N}bps::FEE{N}BPS`).
fn turbos_fee_type_arg(fee_rate: u64) -> Result<String, String> {
    let (module, name) = match fee_rate {
        100 => ("fee100bps", "FEE100BPS"),
        500 => ("fee500bps", "FEE500BPS"),
        3000 => ("fee3000bps", "FEE3000BPS"),
        10000 => ("fee10000bps", "FEE10000BPS"),
        other => {
            return Err(format!(
                "unsupported Turbos fee_rate {other} (no in-package fee type mapping)"
            ));
        }
    };
    // Fee type origins stay on the original discovery package address.
    Ok(format!(
        "{}::{module}::{name}",
        crate::discovery::registry::TURBOS_DISCOVERY_PACKAGE
    ))
}

/// Turbos `swap_*_with_return_` on the upgraded `published-at` package.
/// Returns `(out_coin, leftover_in_coin)`; leftover is transferred to `recipient`.
#[allow(clippy::too_many_arguments)]
fn build_turbos_hop(
    pool: &PoolState,
    current_coin: PtbArgument,
    is_a_to_b: bool,
    expected_amount: u128,
    hop_min_out: u128,
    sqrt_price_limit: u128,
    start_cmd_idx: u16,
    recipient: &str,
    _is_final_hop: bool,
) -> Result<(Vec<PtbCommand>, Option<PtbArgument>), String> {
    let fee_type = turbos_fee_type_arg(pool.fee_rate)?;
    let coin_type = if is_a_to_b {
        pool.coin_type_a.clone()
    } else {
        pool.coin_type_b.clone()
    };
    let function = if is_a_to_b {
        "swap_a_b_with_return_"
    } else {
        "swap_b_a_with_return_"
    };
    let make_vec_idx = start_cmd_idx;
    let swap_idx = start_cmd_idx + 1;

    let make_vec = PtbCommand::MakeMoveVec {
        type_tag: Some(format!("0x2::coin::Coin<{coin_type}>")),
        elements: vec![current_coin],
    };
    let swap = PtbCommand::MoveCall {
        package: package_for(DexSwapKind::Turbos).to_string(),
        module: "swap_router".to_string(),
        function: function.to_string(),
        type_arguments: vec![pool.coin_type_a.clone(), pool.coin_type_b.clone(), fee_type],
        arguments: vec![
            PtbArgument::Object(pool.pool_id.clone()),
            PtbArgument::Result(make_vec_idx),
            PtbArgument::U64(amount_u64(expected_amount)),
            PtbArgument::U64(amount_u64(hop_min_out)),
            PtbArgument::U128(sqrt_price_limit),
            PtbArgument::Bool(true), // exact_in
            PtbArgument::Address(recipient.to_string()),
            PtbArgument::U64(TURBOS_DEADLINE_MS),
            PtbArgument::Object(SUI_CLOCK.to_string()),
            PtbArgument::Object(TURBOS_VERSIONED.to_string()),
        ],
    };
    let transfer_leftover = PtbCommand::TransferObjects {
        objects: vec![PtbArgument::NestedResult(swap_idx, 1)],
        address: PtbArgument::Address(recipient.to_string()),
    };
    Ok((
        vec![make_vec, swap, transfer_leftover],
        Some(PtbArgument::NestedResult(swap_idx, 0)),
    ))
}

#[allow(clippy::too_many_arguments)]
fn build_config_pool_flash_hop(
    package: &str,
    global_config: &str,
    pool: &PoolState,
    current_coin: PtbArgument,
    is_a_to_b: bool,
    expected_amount: u128,
    sqrt_price_limit: u128,
    start_cmd_idx: u16,
) -> (Vec<PtbCommand>, PtbArgument) {
    let package = package.to_string();
    let flash_idx = start_cmd_idx;
    let into_idx = start_cmd_idx + 1;
    let zero_idx = start_cmd_idx + 2;
    // repay at start+3, destroy_zero at start+4, from_balance at start+5
    let from_idx = start_cmd_idx + 5;

    let flash_swap = PtbCommand::MoveCall {
        package: package.clone(),
        module: "pool".to_string(),
        function: "flash_swap".to_string(),
        type_arguments: vec![pool.coin_type_a.clone(), pool.coin_type_b.clone()],
        arguments: vec![
            PtbArgument::Object(global_config.to_string()),
            PtbArgument::Object(pool.pool_id.clone()),
            PtbArgument::Bool(is_a_to_b),
            PtbArgument::Bool(true),
            PtbArgument::U64(amount_u64(expected_amount)),
            PtbArgument::U128(sqrt_price_limit),
            PtbArgument::Object(SUI_CLOCK.to_string()),
        ],
    };

    let pay_type = if is_a_to_b {
        pool.coin_type_a.clone()
    } else {
        pool.coin_type_b.clone()
    };
    let into_balance = PtbCommand::MoveCall {
        package: SUI_FRAMEWORK_PACKAGE.to_string(),
        module: "coin".to_string(),
        function: "into_balance".to_string(),
        type_arguments: vec![pay_type],
        arguments: vec![current_coin],
    };

    let zero_type = if is_a_to_b {
        pool.coin_type_b.clone()
    } else {
        pool.coin_type_a.clone()
    };
    let zero_balance = PtbCommand::MoveCall {
        package: SUI_FRAMEWORK_PACKAGE.to_string(),
        module: "balance".to_string(),
        function: "zero".to_string(),
        type_arguments: vec![zero_type],
        arguments: vec![],
    };

    let receipt = PtbArgument::NestedResult(flash_idx, 2);
    let pay_balance = PtbArgument::Result(into_idx);
    let zero_result = PtbArgument::Result(zero_idx);
    let (balance_a, balance_b) = if is_a_to_b {
        (pay_balance, zero_result)
    } else {
        (zero_result, pay_balance)
    };

    let repay = PtbCommand::MoveCall {
        package: package.clone(),
        module: "pool".to_string(),
        function: "repay_flash_swap".to_string(),
        type_arguments: vec![pool.coin_type_a.clone(), pool.coin_type_b.clone()],
        arguments: vec![
            PtbArgument::Object(global_config.to_string()),
            PtbArgument::Object(pool.pool_id.clone()),
            balance_a,
            balance_b,
            receipt,
        ],
    };

    // flash_swap returns (Balance<A>, Balance<B>, Receipt). Take the output side and
    // destroy_zero the unused side (must not leave a Balance without Drop).
    let (out_balance, unused_balance, unused_type) = if is_a_to_b {
        (
            PtbArgument::NestedResult(flash_idx, 1),
            PtbArgument::NestedResult(flash_idx, 0),
            pool.coin_type_a.clone(),
        )
    } else {
        (
            PtbArgument::NestedResult(flash_idx, 0),
            PtbArgument::NestedResult(flash_idx, 1),
            pool.coin_type_b.clone(),
        )
    };
    let destroy_unused = PtbCommand::MoveCall {
        package: SUI_FRAMEWORK_PACKAGE.to_string(),
        module: "balance".to_string(),
        function: "destroy_zero".to_string(),
        type_arguments: vec![unused_type],
        arguments: vec![unused_balance],
    };

    let out_type = if is_a_to_b {
        pool.coin_type_b.clone()
    } else {
        pool.coin_type_a.clone()
    };
    let from_balance = PtbCommand::MoveCall {
        package: SUI_FRAMEWORK_PACKAGE.to_string(),
        module: "coin".to_string(),
        function: "from_balance".to_string(),
        type_arguments: vec![out_type],
        arguments: vec![out_balance],
    };

    (
        vec![
            flash_swap,
            into_balance,
            zero_balance,
            repay,
            destroy_unused,
            from_balance,
        ],
        PtbArgument::Result(from_idx),
    )
}

fn build_magma_flash_hop(
    pool: &PoolState,
    current_coin: PtbArgument,
    is_a_to_b: bool,
    expected_amount: u128,
    sqrt_price_limit: u128,
    start_cmd_idx: u16,
) -> (Vec<PtbCommand>, PtbArgument) {
    build_config_pool_flash_hop(
        package_for(DexSwapKind::Magma),
        MAGMA_GLOBAL_CONFIG,
        pool,
        current_coin,
        is_a_to_b,
        expected_amount,
        sqrt_price_limit,
        start_cmd_idx,
    )
}

fn build_momentum_flash_hop(
    pool: &PoolState,
    current_coin: PtbArgument,
    is_a_to_b: bool,
    expected_amount: u128,
    sqrt_price_limit: u128,
    start_cmd_idx: u16,
) -> (Vec<PtbCommand>, PtbArgument) {
    let package = package_for(DexSwapKind::Momentum).to_string();
    let flash_idx = start_cmd_idx;
    let into_idx = start_cmd_idx + 1;
    let zero_idx = start_cmd_idx + 2;
    let from_idx = start_cmd_idx + 5;

    let flash_swap = PtbCommand::MoveCall {
        package: package.clone(),
        module: "trade".to_string(),
        function: "flash_swap".to_string(),
        type_arguments: vec![pool.coin_type_a.clone(), pool.coin_type_b.clone()],
        arguments: vec![
            PtbArgument::Object(pool.pool_id.clone()),
            PtbArgument::Bool(is_a_to_b),
            PtbArgument::Bool(true),
            PtbArgument::U64(amount_u64(expected_amount)),
            PtbArgument::U128(sqrt_price_limit),
            PtbArgument::Object(SUI_CLOCK.to_string()),
            PtbArgument::Object(MOMENTUM_VERSION_OBJECT.to_string()),
        ],
    };

    let pay_type = if is_a_to_b {
        pool.coin_type_a.clone()
    } else {
        pool.coin_type_b.clone()
    };
    let into_balance = PtbCommand::MoveCall {
        package: SUI_FRAMEWORK_PACKAGE.to_string(),
        module: "coin".to_string(),
        function: "into_balance".to_string(),
        type_arguments: vec![pay_type],
        arguments: vec![current_coin],
    };

    let zero_type = if is_a_to_b {
        pool.coin_type_b.clone()
    } else {
        pool.coin_type_a.clone()
    };
    let zero_balance = PtbCommand::MoveCall {
        package: SUI_FRAMEWORK_PACKAGE.to_string(),
        module: "balance".to_string(),
        function: "zero".to_string(),
        type_arguments: vec![zero_type],
        arguments: vec![],
    };

    let receipt = PtbArgument::NestedResult(flash_idx, 2);
    let pay_balance = PtbArgument::Result(into_idx);
    let zero_result = PtbArgument::Result(zero_idx);
    let (balance_x, balance_y) = if is_a_to_b {
        (pay_balance, zero_result)
    } else {
        (zero_result, pay_balance)
    };

    let repay = PtbCommand::MoveCall {
        package: package.clone(),
        module: "trade".to_string(),
        function: "repay_flash_swap".to_string(),
        type_arguments: vec![pool.coin_type_a.clone(), pool.coin_type_b.clone()],
        arguments: vec![
            PtbArgument::Object(pool.pool_id.clone()),
            receipt,
            balance_x,
            balance_y,
            PtbArgument::Object(MOMENTUM_VERSION_OBJECT.to_string()),
        ],
    };

    let (out_balance, unused_balance, unused_type) = if is_a_to_b {
        (
            PtbArgument::NestedResult(flash_idx, 1),
            PtbArgument::NestedResult(flash_idx, 0),
            pool.coin_type_a.clone(),
        )
    } else {
        (
            PtbArgument::NestedResult(flash_idx, 0),
            PtbArgument::NestedResult(flash_idx, 1),
            pool.coin_type_b.clone(),
        )
    };
    let destroy_unused = PtbCommand::MoveCall {
        package: SUI_FRAMEWORK_PACKAGE.to_string(),
        module: "balance".to_string(),
        function: "destroy_zero".to_string(),
        type_arguments: vec![unused_type],
        arguments: vec![unused_balance],
    };

    let out_type = if is_a_to_b {
        pool.coin_type_b.clone()
    } else {
        pool.coin_type_a.clone()
    };
    let from_balance = PtbCommand::MoveCall {
        package: SUI_FRAMEWORK_PACKAGE.to_string(),
        module: "coin".to_string(),
        function: "from_balance".to_string(),
        type_arguments: vec![out_type],
        arguments: vec![out_balance],
    };

    (
        vec![
            flash_swap,
            into_balance,
            zero_balance,
            repay,
            destroy_unused,
            from_balance,
        ],
        PtbArgument::Result(from_idx),
    )
}

/// Remaps `NestedResult` / `Result` indices in hop output coin for caller's command offset.
pub fn remap_output_coin(output: PtbArgument, start_cmd_idx: u16) -> PtbArgument {
    match output {
        PtbArgument::NestedResult(cmd, nested) => {
            PtbArgument::NestedResult(start_cmd_idx + cmd, nested)
        }
        PtbArgument::Result(cmd) => PtbArgument::Result(start_cmd_idx + cmd),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::SwapHop;

    fn sample_pool(dex_name: &str) -> PoolState {
        PoolState {
            pool_id: format!("0x_{dex_name}_pool"),
            dex_name: dex_name.to_string(),
            coin_type_a: "0x2::sui::SUI".to_string(),
            coin_type_b: "0xusdc::coin::COIN".to_string(),
            sqrt_price: 1 << 64,
            liquidity: 1_000_000,
            fee_rate: 3000,
            is_paused: false,
        }
    }

    fn sample_hop(dex_name: &str, a_to_b: bool) -> SwapHop {
        let pool = sample_pool(dex_name);
        SwapHop {
            pool_id: pool.pool_id.clone(),
            dex_name: dex_name.to_string(),
            input_token: if a_to_b {
                pool.coin_type_a.clone()
            } else {
                pool.coin_type_b.clone()
            },
            output_token: if a_to_b {
                pool.coin_type_b
            } else {
                pool.coin_type_a
            },
            fee_rate: 3000,
        }
    }

    fn move_call(cmd: &PtbCommand) -> (&str, &str, &str, &[String], &[PtbArgument]) {
        match cmd {
            PtbCommand::MoveCall {
                package,
                module,
                function,
                type_arguments,
                arguments,
            } => (
                package.as_str(),
                module.as_str(),
                function.as_str(),
                type_arguments,
                arguments,
            ),
            _ => panic!("expected MoveCall"),
        }
    }

    #[test]
    fn test_dex_kind_detection() {
        assert_eq!(dex_kind("Cetus"), DexSwapKind::Cetus);
        assert_eq!(dex_kind("Magma Finance"), DexSwapKind::Magma);
        assert_eq!(dex_kind("Momentum"), DexSwapKind::Momentum);
        assert_eq!(dex_kind("Unknown"), DexSwapKind::Unsupported);
    }

    #[test]
    fn test_cetus_hop_uses_flash_swap_pattern() {
        let pool = sample_pool("Cetus");
        let hop = sample_hop("Cetus", true);
        let (cmds, out) = build_hop_commands(
            &pool,
            &hop,
            PtbArgument::NestedResult(0, 0),
            1000,
            990,
            MIN_SQRT_PRICE.parse().unwrap(),
            1,
            "0xabc",
            true,
        )
        .unwrap();
        assert_eq!(cmds.len(), 6);
        let (pkg, module, fun, _, args) = move_call(&cmds[0]);
        assert_eq!(pkg, CETUS_PACKAGE);
        assert_eq!(module, "pool");
        assert_eq!(fun, "flash_swap");
        assert_eq!(args[0], PtbArgument::Object(CETUS_CONFIG.to_string()));
        assert_eq!(args[2], PtbArgument::Bool(true));
        assert_eq!(args[4], PtbArgument::U64(1000));
        assert_eq!(out, Some(PtbArgument::Result(6)));
    }

    #[test]
    fn test_magma_flash_hop_includes_balance_conversions() {
        let pool = sample_pool("Magma Finance");
        let hop = sample_hop("Magma Finance", true);
        let input = PtbArgument::NestedResult(0, 0);
        let (cmds, out) = build_hop_commands(
            &pool,
            &hop,
            input,
            5000,
            0,
            MIN_SQRT_PRICE.parse().unwrap(),
            2,
            "0xabc",
            true,
        )
        .unwrap();
        assert_eq!(cmds.len(), 6);
        let (pkg0, _, fun0, _, args0) = move_call(&cmds[0]);
        assert_eq!(pkg0, MAGMA_PACKAGE);
        assert_eq!(fun0, "flash_swap");
        assert_eq!(
            args0[0],
            PtbArgument::Object(MAGMA_GLOBAL_CONFIG.to_string())
        );

        let (_, mod1, fun1, _, _) = move_call(&cmds[1]);
        assert_eq!(mod1, "coin");
        assert_eq!(fun1, "into_balance");

        let (_, mod4, fun4, _, _) = move_call(&cmds[4]);
        assert_eq!(mod4, "balance");
        assert_eq!(fun4, "destroy_zero");

        let (_, mod5, fun5, _, _) = move_call(&cmds[5]);
        assert_eq!(mod5, "coin");
        assert_eq!(fun5, "from_balance");
        assert_eq!(out, Some(PtbArgument::Result(7)));
    }

    #[test]
    fn test_momentum_flash_hop_a2b() {
        let pool = sample_pool("Momentum");
        let hop = sample_hop("Momentum", true);
        let (cmds, out) = build_hop_commands(
            &pool,
            &hop,
            PtbArgument::NestedResult(0, 0),
            3000,
            0,
            MIN_SQRT_PRICE.parse().unwrap(),
            1,
            "0xabc",
            true,
        )
        .unwrap();
        assert_eq!(cmds.len(), 6);
        let (pkg0, mod0, fun0, _, _) = move_call(&cmds[0]);
        assert_eq!(pkg0, MOMENTUM_PACKAGE);
        assert_eq!(mod0, "trade");
        assert_eq!(fun0, "flash_swap");
        assert_eq!(out, Some(PtbArgument::Result(6)));
    }

    #[test]
    fn test_turbos_hop_uses_swap_router_with_return() {
        let pool = sample_pool("Turbos");
        let hop = sample_hop("Turbos", true);
        let (cmds, out) = build_hop_commands(
            &pool,
            &hop,
            PtbArgument::NestedResult(0, 0),
            1000,
            990,
            MIN_SQRT_PRICE.parse().unwrap(),
            1,
            "0xdead",
            true,
        )
        .unwrap();
        assert_eq!(cmds.len(), 3);
        assert!(matches!(cmds[0], PtbCommand::MakeMoveVec { .. }));
        let (pkg, module, fun, types, args) = move_call(&cmds[1]);
        assert_eq!(pkg, TURBOS_PACKAGE);
        assert_eq!(module, "swap_router");
        assert_eq!(fun, "swap_a_b_with_return_");
        assert_eq!(types.len(), 3);
        assert!(types[2].contains("fee3000bps"));
        assert!(types[2].starts_with(crate::discovery::registry::TURBOS_DISCOVERY_PACKAGE));
        assert_eq!(args[6], PtbArgument::Address("0xdead".to_string()));
        assert_eq!(args[9], PtbArgument::Object(TURBOS_VERSIONED.to_string()));
        assert!(matches!(cmds[2], PtbCommand::TransferObjects { .. }));
        assert_eq!(out, Some(PtbArgument::NestedResult(2, 0)));
    }

    #[test]
    fn test_dynamic_sqrt_limit_passed_to_move_call() {
        let pool = sample_pool("Cetus");
        let hop = sample_hop("Cetus", true);
        let dynamic_limit = 5_000_000_000_000u128;
        let (cmds, _) = build_hop_commands(
            &pool,
            &hop,
            PtbArgument::NestedResult(0, 0),
            1000,
            990,
            dynamic_limit,
            0,
            "0xabc",
            true,
        )
        .unwrap();
        let (_, _, _, _, args) = move_call(&cmds[0]);
        // flash_swap args: config, pool, a2b, by_amount_in, amount, sqrt_price_limit, clock
        assert_eq!(args[5], PtbArgument::U128(dynamic_limit));
    }

    #[test]
    fn test_remap_output_coin() {
        assert_eq!(
            remap_output_coin(PtbArgument::NestedResult(0, 1), 3),
            PtbArgument::NestedResult(3, 1)
        );
        assert_eq!(
            remap_output_coin(PtbArgument::Result(2), 3),
            PtbArgument::Result(5)
        );
    }

    #[test]
    fn test_mixed_route_command_chain() {
        let cetus_pool = sample_pool("Cetus");
        let momentum_pool = sample_pool("Momentum");
        let cetus_hop = sample_hop("Cetus", true);
        let momentum_hop = SwapHop {
            pool_id: momentum_pool.pool_id.clone(),
            dex_name: "Momentum".to_string(),
            input_token: momentum_pool.coin_type_a.clone(),
            output_token: momentum_pool.coin_type_b.clone(),
            fee_rate: 3000,
        };
        // Force USDC->SUI direction for second hop semantics in mixed path test:
        let momentum_hop = SwapHop {
            input_token: cetus_pool.coin_type_b.clone(),
            output_token: cetus_pool.coin_type_a.clone(),
            ..momentum_hop
        };

        let mut commands: Vec<PtbCommand> = Vec::new();
        let split_coin = PtbArgument::NestedResult(0, 0);

        let (cetus_cmds, coin_after_cetus) = build_hop_commands(
            &cetus_pool,
            &cetus_hop,
            split_coin,
            1_000_000,
            990_000,
            MIN_SQRT_PRICE.parse().unwrap(),
            commands.len() as u16,
            "0xabc",
            false,
        )
        .unwrap();
        commands.extend(cetus_cmds);
        let coin_after_cetus = coin_after_cetus.expect("cetus returns coin");

        let (mom_cmds, final_coin) = build_hop_commands(
            &momentum_pool,
            &momentum_hop,
            coin_after_cetus,
            500_000,
            0,
            MAX_SQRT_PRICE.parse().unwrap(),
            commands.len() as u16,
            "0xabc",
            true,
        )
        .unwrap();
        commands.extend(mom_cmds);

        assert_eq!(commands.len(), 12); // 6 cetus flash + 6 momentum flash
        let (_, _, fun0, _, _) = move_call(&commands[0]);
        assert_eq!(fun0, "flash_swap");
        let (_, mod6, fun6, _, _) = move_call(&commands[6]);
        assert_eq!(mod6, "trade");
        assert_eq!(fun6, "flash_swap");
        assert_eq!(final_coin, Some(PtbArgument::Result(11)));
    }
}
