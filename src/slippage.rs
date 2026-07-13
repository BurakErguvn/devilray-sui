//! Basis-point slippage budget and dynamic sqrt-price limit helpers.
//!
//! Amount math uses `u128` only; tolerance is normalized from `f64` at the API boundary.

use crate::dex_swap::{MAX_SQRT_PRICE, MIN_SQRT_PRICE};
use std::fmt;

const BPS_SCALE: u32 = 10_000;

/// Validated slippage tolerance in basis points (`0..10_000`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlippageBps(u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlippageError {
    NotFinite,
    OutOfRange,
}

impl fmt::Display for SlippageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SlippageError::NotFinite => write!(f, "slippage_tolerance must be finite"),
            SlippageError::OutOfRange => {
                write!(f, "slippage_tolerance must satisfy 0 <= value < 1")
            }
        }
    }
}

impl std::error::Error for SlippageError {}

impl SlippageBps {
    /// Normalizes API tolerance to basis points. Rejects NaN, infinity, negatives, and `>= 1`.
    pub fn try_from_tolerance(tolerance: f64) -> Result<Self, SlippageError> {
        if !tolerance.is_finite() {
            return Err(SlippageError::NotFinite);
        }
        if !(0.0..1.0).contains(&tolerance) {
            return Err(SlippageError::OutOfRange);
        }
        let bps = (tolerance * BPS_SCALE as f64).round() as u32;
        Ok(Self(bps.min(BPS_SCALE - 1)))
    }

    pub fn bps(&self) -> u32 {
        self.0
    }

    pub fn retain_bps(&self) -> u32 {
        BPS_SCALE - self.0
    }

    /// `floor(amount * retain_bps / 10_000)` — overflow-safe for practical swap sizes.
    pub fn apply_to_amount(&self, amount: u128) -> u128 {
        apply_retain_bps(amount, self.retain_bps())
    }

    /// Compound retain after `hop_one_based` hops out of `total_hops`: `(1 - T)^(i/n)`.
    pub fn compound_retain_bps(&self, hop_one_based: u32, total_hops: u32) -> u32 {
        compound_retain_bps(self.0, hop_one_based, total_hops)
    }

    /// Per-hop incremental retain for intermediate hops: `(1 - T)^(1/n)`.
    pub fn incremental_hop_retain_bps(&self, total_hops: u32) -> u32 {
        if total_hops <= 1 {
            return self.retain_bps();
        }
        self.compound_retain_bps(1, total_hops)
    }

    /// Adverse slippage budget (bps) for a hop in a multi-hop path.
    pub fn hop_adverse_bps(&self, hop_idx: usize, total_hops: usize) -> u32 {
        if total_hops <= 1 || hop_idx + 1 == total_hops {
            self.bps()
        } else {
            BPS_SCALE - self.incremental_hop_retain_bps(total_hops as u32)
        }
    }
}

pub fn apply_retain_bps(amount: u128, retain_bps: u32) -> u128 {
    let retain = retain_bps as u128;
    let scale = BPS_SCALE as u128;
    amount / scale * retain + amount % scale * retain / scale
}

/// Compound retain factor in basis points: `floor(10_000 * (1 - T)^(num/den))`.
pub fn compound_retain_bps(total_slippage_bps: u32, hop_one_based: u32, total_hops: u32) -> u32 {
    if total_hops == 0 || hop_one_based == 0 {
        return BPS_SCALE;
    }
    let retain_ratio = (BPS_SCALE - total_slippage_bps) as f64 / BPS_SCALE as f64;
    let exponent = hop_one_based as f64 / total_hops as f64;
    let factor = retain_ratio.powf(exponent);
    (factor * BPS_SCALE as f64).floor() as u32
}

/// Minimum path output after full user tolerance.
pub fn path_min_amount_out(total_out: u128, slippage: &SlippageBps) -> u128 {
    slippage.apply_to_amount(total_out)
}

/// Per-hop minimum output for an intermediate hop (compound `1/n` budget).
pub fn intermediate_hop_min_out(
    expected_out: u128,
    slippage: &SlippageBps,
    total_hops: u32,
) -> u128 {
    let retain = slippage.incremental_hop_retain_bps(total_hops);
    apply_retain_bps(expected_out, retain)
}

const MIN_SQRT_PRICE_F64: f64 = 4_295_048_016.0;
const MAX_SQRT_PRICE_F64: f64 = 79_226_673_515_401_279_992_447_579_055.0;

/// Dynamic sqrt price limit (raw on-chain scale) from simulated final sqrt and hop budget.
pub fn dynamic_sqrt_price_limit_raw(
    final_sqrt_internal: f64,
    factor_bits: i32,
    is_a_to_b: bool,
    adverse_slippage_bps: u32,
) -> u128 {
    if !final_sqrt_internal.is_finite() || final_sqrt_internal <= 0.0 {
        return static_sqrt_price_limit(is_a_to_b);
    }

    let scale = 2.0f64.powi(factor_bits);
    let adverse_frac = adverse_slippage_bps as f64 / BPS_SCALE as f64;
    let limit_internal = if is_a_to_b {
        final_sqrt_internal * (1.0 - adverse_frac)
    } else {
        final_sqrt_internal * (1.0 + adverse_frac)
    };

    let mut raw = (limit_internal * scale).floor();
    let final_raw = final_sqrt_internal * scale;
    if is_a_to_b {
        raw = raw.min(final_raw);
    } else {
        raw = raw.max(final_raw);
    }

    let clamped = raw.clamp(MIN_SQRT_PRICE_F64, MAX_SQRT_PRICE_F64) as u128;
    let min_u128: u128 = MIN_SQRT_PRICE.parse().unwrap();
    let max_u128: u128 = MAX_SQRT_PRICE.parse().unwrap();
    clamped.clamp(min_u128, max_u128)
}

fn static_sqrt_price_limit(is_a_to_b: bool) -> u128 {
    if is_a_to_b {
        MIN_SQRT_PRICE.parse().unwrap()
    } else {
        MAX_SQRT_PRICE.parse().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slippage_validation() {
        assert_eq!(SlippageBps::try_from_tolerance(0.0).unwrap().bps(), 0);
        assert_eq!(SlippageBps::try_from_tolerance(0.01).unwrap().bps(), 100);
        assert_eq!(SlippageBps::try_from_tolerance(0.9999).unwrap().bps(), 9999);
        assert!(SlippageBps::try_from_tolerance(-0.1).is_err());
        assert!(SlippageBps::try_from_tolerance(1.0).is_err());
        assert!(SlippageBps::try_from_tolerance(f64::NAN).is_err());
        assert!(SlippageBps::try_from_tolerance(f64::INFINITY).is_err());
    }

    #[test]
    fn test_apply_to_amount() {
        let s = SlippageBps::try_from_tolerance(0.01).unwrap();
        assert_eq!(s.apply_to_amount(1_000_000), 990_000);
        assert_eq!(s.apply_to_amount(0), 0);
        assert_eq!(
            SlippageBps::try_from_tolerance(0.0)
                .unwrap()
                .apply_to_amount(42),
            42
        );
    }

    #[test]
    fn test_compound_two_hop_budget() {
        let s = SlippageBps::try_from_tolerance(0.05).unwrap();
        let r1 = s.compound_retain_bps(1, 2);
        let r2 = s.compound_retain_bps(2, 2);
        assert!(r1 > r2);
        assert!(r2 >= s.retain_bps());
        let incremental = s.incremental_hop_retain_bps(2);
        assert_eq!(incremental, r1);
    }

    #[test]
    fn test_compound_three_hop_budget() {
        let s = SlippageBps::try_from_tolerance(0.01).unwrap();
        let r1 = s.compound_retain_bps(1, 3);
        let r2 = s.compound_retain_bps(2, 3);
        let r3 = s.compound_retain_bps(3, 3);
        assert!(r1 > r2 && r2 > r3);
        assert_eq!(r3, s.retain_bps());
    }

    #[test]
    fn test_intermediate_hop_min_out() {
        let s = SlippageBps::try_from_tolerance(0.05).unwrap();
        let min = intermediate_hop_min_out(1_000_000, &s, 3);
        assert!(min > s.apply_to_amount(1_000_000));
        assert!(min < 1_000_000);
    }

    #[test]
    fn test_u128_boundary_apply() {
        let s = SlippageBps::try_from_tolerance(0.0001).unwrap();
        let large = u128::MAX / 2;
        let out = s.apply_to_amount(large);
        assert!(out < large);
        assert!(out > 0);
    }

    #[test]
    fn test_dynamic_sqrt_limit_direction() {
        let final_sqrt = 1.5;
        let a2b = dynamic_sqrt_price_limit_raw(final_sqrt, 64, true, 100);
        let b2a = dynamic_sqrt_price_limit_raw(final_sqrt, 64, false, 100);
        let scale = 2.0f64.powi(64);
        assert!(a2b < (final_sqrt * scale) as u128);
        assert!(b2a > (final_sqrt * scale) as u128);
    }

    #[test]
    fn test_dynamic_sqrt_limit_clamps() {
        let tiny = dynamic_sqrt_price_limit_raw(1e-20, 64, true, 5000);
        assert!(tiny >= MIN_SQRT_PRICE.parse::<u128>().unwrap());
        let high = dynamic_sqrt_price_limit_raw(1.0, 64, false, 9999);
        assert!(high <= MAX_SQRT_PRICE.parse::<u128>().unwrap());
    }
}
