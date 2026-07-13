//! Sui Move signed integer bit encoding helpers.
//!
//! - **Cetus / Magma `liquidity_net`**: `integer_mate` (`bits >> 2`, sign `0b01`).
//! - **Turbos / Magma / Momentum tick indices**: raw two's-complement in `bits` (`bits as i32`).

/// Decodes Cetus `integer_mate::i32` from raw `bits` field.
pub fn decode_sui_i32(bits: u32) -> i32 {
    let mag = (bits >> 2) as i32;
    if (bits & 0b11) == 0b01 { -mag } else { mag }
}

/// Decodes Cetus `integer_mate::i128` from raw `bits` field (lower 128 bits).
/// Decodes Turbos/Magma/Momentum tick index from raw `bits` (two's-complement i32).
pub fn decode_raw_i32(bits: u32) -> i32 {
    bits as i32
}

/// Encodes a tick index for Turbos/Magma/Momentum `I32::bits` dynamic-field keys.
pub fn encode_raw_i32(val: i32) -> u32 {
    val as u32
}

pub fn decode_sui_i128(bits: u128) -> i128 {
    let mag = (bits >> 2) as i128;
    if (bits & 0b11) == 0b01 { -mag } else { mag }
}

/// Extracts `bits` from a JSON value that may be a string, number, or `{ "fields": { "bits": ... } }`.
pub fn parse_i32_bits_from_json(val: &serde_json::Value) -> Option<u32> {
    if let Some(s) = val.as_str() {
        return s.parse().ok();
    }
    if let Some(n) = val.as_u64() {
        return Some(n as u32);
    }
    if let Some(fields) = val.get("fields")
        && let Some(bits) = fields.get("bits")
    {
        return parse_i32_bits_from_json(bits);
    }
    if let Some(bits) = val.get("bits") {
        return parse_i32_bits_from_json(bits);
    }
    None
}

/// Extracts i128 `bits` from JSON (string or nested fields).
pub fn parse_i128_bits_from_json(val: &serde_json::Value) -> Option<u128> {
    if let Some(s) = val.as_str() {
        return s.parse().ok();
    }
    if let Some(fields) = val.get("fields")
        && let Some(bits) = fields.get("bits")
    {
        return parse_i128_bits_from_json(bits);
    }
    if let Some(bits) = val.get("bits") {
        return parse_i128_bits_from_json(bits);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_decode_sui_i32_positive() {
        // magnitude 10 -> bits = (10 << 2) | 0 = 40
        assert_eq!(decode_sui_i32(40), 10);
    }

    #[test]
    fn test_decode_sui_i32_negative() {
        // magnitude 10, negative -> (10 << 2) | 1 = 41
        assert_eq!(decode_sui_i32(41), -10);
    }

    #[test]
    fn test_decode_sui_i32_zero() {
        assert_eq!(decode_sui_i32(0), 0);
    }

    #[test]
    fn test_decode_sui_i128_positive() {
        let bits: u128 = (500_000u128) << 2;
        assert_eq!(decode_sui_i128(bits), 500_000);
    }

    #[test]
    fn test_decode_sui_i128_negative() {
        let bits: u128 = ((500_000u128) << 2) | 1;
        assert_eq!(decode_sui_i128(bits), -500_000);
    }

    #[test]
    fn test_parse_i32_bits_nested() {
        let val = json!({ "type": "0x...::i32::I32", "fields": { "bits": 1255 } });
        assert_eq!(parse_i32_bits_from_json(&val), Some(1255));
        assert_eq!(decode_sui_i32(1255), 313);
    }

    #[test]
    fn test_decode_raw_i32_negative_tick() {
        // Turbos SUI/USDC mainnet tick_current_index encoding (verified via probe_ticks).
        assert_eq!(decode_raw_i32(4_294_895_002), -72_294);
    }

    #[test]
    fn test_encode_raw_i32_roundtrip() {
        for tick in [-72_294i32, -2, 0, 31_360, 72_388] {
            assert_eq!(decode_raw_i32(encode_raw_i32(tick)), tick);
        }
    }
}
