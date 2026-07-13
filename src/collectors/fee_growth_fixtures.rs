//! Senior acceptance fixtures for pool-level `fee_growth_global` field aliases (Faz 0).
//!
//! | DEX      | On-chain aliases                         | Normalized model |
//! |----------|------------------------------------------|------------------|
//! | Cetus    | `fee_growth_global_a`, `fee_growth_global_b` | a / b          |
//! | Turbos   | `fee_growth_global_a`, `fee_growth_global_b` | a / b          |
//! | Magma    | `fee_growth_global_a`, `fee_growth_global_b` | a / b          |
//! | Momentum | `fee_growth_global_x`, `fee_growth_global_y` | a / b          |
//!
//! Missing fields deserialize as `None`; malformed present fields are parse errors.

use serde_json::json;

pub fn cetus_pool_fields() -> serde_json::Value {
    json!({
        "fee_growth_global_a": { "type": "0x2::u128::U128", "fields": { "bits": "12345678901234567890" } },
        "fee_growth_global_b": "98765432109876543210"
    })
}

pub fn turbos_pool_fields() -> serde_json::Value {
    json!({
        "fee_growth_global_a": "11111111111111111111",
        "fee_growth_global_b": { "fields": { "bits": "22222222222222222222" } }
    })
}

pub fn magma_pool_fields() -> serde_json::Value {
    json!({
        "fee_growth_global_a": "33333333333333333333",
        "fee_growth_global_b": "44444444444444444444"
    })
}

pub fn momentum_pool_fields() -> serde_json::Value {
    json!({
        "fee_growth_global_x": "55555555555555555555",
        "fee_growth_global_y": "66666666666666666666"
    })
}

pub fn pool_fields_missing_fee_growth() -> serde_json::Value {
    json!({
        "tick_spacing": 60,
        "current_tick_index": { "fields": { "bits": 0 } }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collectors::tick_fetch::parse_fee_growth_global;

    #[test]
    fn test_cetus_fee_growth_fixture() {
        let (a, b) = parse_fee_growth_global(&cetus_pool_fields()).unwrap();
        assert_eq!(a, Some(12_345_678_901_234_567_890));
        assert_eq!(b, Some(98_765_432_109_876_543_210));
    }

    #[test]
    fn test_turbos_fee_growth_fixture() {
        let (a, b) = parse_fee_growth_global(&turbos_pool_fields()).unwrap();
        assert_eq!(a, Some(11_111_111_111_111_111_111));
        assert_eq!(b, Some(22_222_222_222_222_222_222));
    }

    #[test]
    fn test_magma_fee_growth_fixture() {
        let (a, b) = parse_fee_growth_global(&magma_pool_fields()).unwrap();
        assert_eq!(a, Some(33_333_333_333_333_333_333));
        assert_eq!(b, Some(44_444_444_444_444_444_444));
    }

    #[test]
    fn test_momentum_fee_growth_fixture_normalized() {
        let (a, b) = parse_fee_growth_global(&momentum_pool_fields()).unwrap();
        assert_eq!(a, Some(55_555_555_555_555_555_555));
        assert_eq!(b, Some(66_666_666_666_666_666_666));
    }

    #[test]
    fn test_missing_fee_growth_is_none() {
        let (a, b) = parse_fee_growth_global(&pool_fields_missing_fee_growth()).unwrap();
        assert_eq!(a, None);
        assert_eq!(b, None);
    }

    #[test]
    fn test_malformed_fee_growth_errors() {
        let bad = json!({ "fee_growth_global_a": { "fields": { "bits": "not_a_number" } } });
        assert!(parse_fee_growth_global(&bad).is_err());
    }
}
