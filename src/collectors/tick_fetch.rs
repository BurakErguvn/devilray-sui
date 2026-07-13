//! Shared tick-fetch helpers for CLMM collectors.

use crate::collectors::MAX_TICK_FETCH_WINDOW;
use crate::models::{PoolTickData, TickInfo};
use crate::sui_client::SuiClientTrait;
use crate::sui_int::{
    decode_raw_i32, decode_sui_i32, decode_sui_i128, encode_raw_i32, parse_i32_bits_from_json,
    parse_i128_bits_from_json,
};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};

/// Normalizes JSON-RPC or GraphQL object responses into `(fields, type)`.
pub fn parse_move_object_response(val: &Value) -> Result<(Value, String)> {
    if let Some(content) = val.get("data").and_then(|d| d.get("content")) {
        let fields = content
            .get("fields")
            .ok_or_else(|| anyhow!("Missing 'fields' in content"))?;
        let type_str = content
            .get("type")
            .and_then(|t| t.as_str())
            .ok_or_else(|| anyhow!("Missing 'type' in content"))?;
        return Ok((fields.clone(), type_str.to_string()));
    }

    if let Some(as_move) = val.get("object").and_then(|o| o.get("asMoveObject")) {
        let fields = as_move
            .get("contents")
            .and_then(|c| c.get("json"))
            .ok_or_else(|| anyhow!("Missing 'contents.json' in GraphQL response"))?;
        let type_str = as_move
            .get("type")
            .and_then(|t| t.get("repr"))
            .and_then(|r| r.as_str())
            .ok_or_else(|| anyhow!("Missing 'type.repr' in GraphQL response"))?;
        return Ok((fields.clone(), type_str.to_string()));
    }

    if let Some(type_str) = val.get("type").and_then(|t| t.as_str())
        && let Some(fields) = val.get("fields")
    {
        return Ok((fields.clone(), type_str.to_string()));
    }

    Err(anyhow!("Unsupported response format: {:?}", val))
}

pub fn parse_u32_field(fields: &Value, keys: &[&str]) -> Result<u32> {
    for key in keys {
        if let Some(val) = fields.get(*key) {
            if let Some(s) = val.as_str() {
                return Ok(s.parse::<u32>()?);
            }
            if let Some(n) = val.as_u64() {
                return Ok(n as u32);
            }
        }
    }
    Err(anyhow!("missing u32 field {:?}", keys))
}

/// Parses a single JSON value as `u128` (plain string/number or integer_mate `bits`).
pub fn parse_u128_value(val: &Value) -> Result<u128> {
    if let Some(s) = val.as_str() {
        return Ok(s.parse::<u128>()?);
    }
    if let Some(n) = val.as_u64() {
        return Ok(n as u128);
    }
    if let Some(bits) = val
        .get("fields")
        .and_then(|f| f.get("bits"))
        .and_then(|b| b.as_str())
    {
        return Ok(bits.parse::<u128>()?);
    }
    if let Some(bits) = val
        .get("fields")
        .and_then(|f| f.get("bits"))
        .and_then(|b| b.as_u64())
    {
        return Ok(bits as u128);
    }
    Err(anyhow!("unsupported u128 encoding: {:?}", val))
}

/// Parses top-level generic type arguments without splitting nested `<...>` groups.
pub fn parse_top_level_generic_args(type_str: &str, expected: usize) -> Result<Vec<String>> {
    let start = type_str
        .find('<')
        .ok_or_else(|| anyhow!("Invalid pool type format: missing '<'"))?;
    let mut args = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for ch in type_str[start + 1..].chars() {
        match ch {
            '<' => {
                depth += 1;
                current.push(ch);
            }
            '>' => {
                if depth == 0 {
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        args.push(trimmed.to_string());
                    }
                    break;
                }
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    args.push(trimmed.to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if args.len() < expected {
        return Err(anyhow!(
            "expected at least {expected} generic parameters, found {}",
            args.len()
        ));
    }
    Ok(args)
}

/// Parses coin types from pool generic parameters (supports nested generics).
pub fn parse_coin_types_from_pool_type(type_str: &str) -> Result<(String, String)> {
    let args = parse_top_level_generic_args(type_str, 2)?;
    Ok((args[0].clone(), args[1].clone()))
}

pub fn parse_optional_u128_field(fields: &Value, keys: &[&str]) -> Result<Option<u128>> {
    for key in keys {
        match fields.get(*key) {
            None => continue,
            Some(Value::Null) => return Ok(None),
            Some(val) => return Ok(Some(parse_u128_value(val)?)),
        }
    }
    Ok(None)
}

/// Normalizes pool-level fee growth to `(a, b)`. Momentum `x/y` aliases map to `a/b`.
pub fn parse_fee_growth_global(fields: &Value) -> Result<(Option<u128>, Option<u128>)> {
    let a = parse_optional_u128_field(fields, &["fee_growth_global_a"])?;
    let b = parse_optional_u128_field(fields, &["fee_growth_global_b"])?;
    if a.is_some() || b.is_some() {
        return Ok((a, b));
    }
    let x = parse_optional_u128_field(fields, &["fee_growth_global_x"])?;
    let y = parse_optional_u128_field(fields, &["fee_growth_global_y"])?;
    Ok((x, y))
}

pub fn parse_cetus_tick_index(fields: &Value) -> Result<i32> {
    let tick_val = fields
        .get("current_tick_index")
        .ok_or_else(|| anyhow!("pool missing current_tick_index"))?;
    let bits =
        parse_i32_bits_from_json(tick_val).ok_or_else(|| anyhow!("tick index missing bits"))?;
    Ok(decode_sui_i32(bits))
}

pub fn parse_raw_tick_index(fields: &Value, keys: &[&str]) -> Result<i32> {
    for key in keys {
        if let Some(tick_val) = fields.get(*key)
            && let Some(bits) = parse_i32_bits_from_json(tick_val)
        {
            return Ok(decode_raw_i32(bits));
        }
    }
    Err(anyhow!("pool missing raw tick index {:?}", keys))
}

pub fn extract_nested_object_id(node: &Value) -> Option<String> {
    if let Some(s) = node.as_str() {
        return Some(s.to_string());
    }
    if let Some(id_obj) = node.get("id") {
        if let Some(s) = id_obj.as_str() {
            return Some(s.to_string());
        }
        if let Some(inner) = id_obj.get("id").and_then(|v| v.as_str()) {
            return Some(inner.to_string());
        }
    }
    if let Some(fields) = node.get("fields")
        && let Some(id) = extract_nested_object_id(fields)
    {
        return Some(id);
    }
    None
}

pub fn tick_in_window(tick_index: i32, current: i32, window: i32) -> bool {
    tick_index >= current - window && tick_index <= current + window
}

pub fn div_floor(a: i32, b: i32) -> i32 {
    debug_assert_ne!(b, 0);
    let d = a / b;
    let r = a % b;
    if r != 0 && (r > 0) != (b > 0) {
        d - 1
    } else {
        d
    }
}

/// Aligned tick indices (multiples of `spacing`) inside ±`window` around `current`.
pub fn aligned_ticks_in_window(current: i32, spacing: i32, window: i32) -> Vec<i32> {
    if spacing <= 0 {
        return Vec::new();
    }
    let low = current - window;
    let high = current + window;
    let start = div_floor(low, spacing) * spacing;
    let mut ticks = Vec::new();
    let mut t = start;
    while t <= high {
        if t >= low {
            ticks.push(t);
        }
        t += spacing;
    }
    ticks
}

pub fn parse_tick_index_from_df_name(name: &Value) -> Option<i32> {
    let bits = parse_i32_bits_from_json(name.get("value").unwrap_or(name))?;
    Some(decode_sui_i32(bits))
}

pub fn parse_raw_tick_index_from_df_name(name: &Value) -> Option<i32> {
    let bits = parse_i32_bits_from_json(name.get("value").unwrap_or(name))?;
    Some(decode_raw_i32(bits))
}

pub fn parse_liquidity_net_from_fields(fields: &Value) -> Option<i128> {
    let ln = fields.get("liquidity_net")?;
    let bits = parse_i128_bits_from_json(ln)?;
    Some(decode_sui_i128(bits))
}

pub fn parse_liquidity_net_from_tick_object(obj: &Value) -> Option<i128> {
    if let Some(fields) = obj
        .get("data")
        .and_then(|d| d.get("content"))
        .and_then(|c| c.get("fields"))
    {
        if let Some(ln) = parse_liquidity_net_from_fields(fields) {
            return Some(ln);
        }
        if let Some(inner) = fields.get("value").and_then(|v| v.get("fields")) {
            return parse_liquidity_net_from_fields(inner);
        }
    }
    None
}

fn dig<'a>(node: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cur = node;
    for key in path {
        cur = cur.get(key)?;
    }
    Some(cur)
}

fn tick_payload_fields(fields: &Value) -> Option<&Value> {
    const PATHS: &[&[&str]] = &[
        &["value", "fields", "value", "fields", "value", "fields"],
        &["value", "fields", "value", "fields"],
        &["value", "fields"],
    ];
    for path in PATHS {
        if let Some(candidate) = dig(fields, path)
            && candidate.get("liquidity_net").is_some()
        {
            return Some(candidate);
        }
    }
    if fields.get("liquidity_net").is_some() {
        Some(fields)
    } else {
        None
    }
}

pub fn parse_skip_list_tick_object(obj: &Value) -> Option<(i32, i128)> {
    let fields = obj
        .get("data")
        .and_then(|d| d.get("content"))
        .and_then(|c| c.get("fields"))?;
    let tick_fields = tick_payload_fields(fields)?;
    let index = parse_raw_tick_index(tick_fields, &["index", "tick_index"]).ok()?;
    let liquidity_net = parse_liquidity_net_from_fields(tick_fields)?;
    Some((index, liquidity_net))
}

pub fn parse_table_tick_object(obj: &Value) -> Option<(i32, i128)> {
    let fields = obj
        .get("data")
        .and_then(|d| d.get("content"))
        .and_then(|c| c.get("fields"))?;
    let tick_index = fields
        .get("name")
        .and_then(parse_raw_tick_index_from_df_name)?;
    let tick_fields = fields
        .get("value")
        .and_then(|v| v.get("fields"))
        .or_else(|| tick_payload_fields(fields))?;
    let liquidity_net = parse_liquidity_net_from_fields(tick_fields)?;
    Some((tick_index, liquidity_net))
}

/// Cetus-style: ticks as dynamic fields on the pool parent, DF name = `integer_mate` I32.
pub async fn collect_ticks_from_integer_mate_df(
    client: &dyn SuiClientTrait,
    parent_id: &str,
    current_tick_index: i32,
    window: i32,
) -> Result<Vec<TickInfo>> {
    let mut ticks = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0u32;

    while pages < 10 {
        pages += 1;
        let page = client
            .get_dynamic_fields(parent_id, cursor.as_deref(), Some(50))
            .await?;

        let data = page
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        if data.is_empty() {
            break;
        }

        for entry in &data {
            let name = match entry.get("name") {
                Some(n) => n,
                None => continue,
            };
            let tick_index = match parse_tick_index_from_df_name(name) {
                Some(t) => t,
                None => continue,
            };
            if !tick_in_window(tick_index, current_tick_index, window) {
                continue;
            }

            let object_id = entry
                .get("objectId")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if object_id.is_empty() {
                continue;
            }

            let tick_obj = match client.get_object(object_id).await {
                Ok(o) => o,
                Err(e) => {
                    tracing::warn!(
                        "Failed to fetch tick object {} for parent {}: {:?}",
                        object_id,
                        parent_id,
                        e
                    );
                    continue;
                }
            };

            let liquidity_net = match parse_liquidity_net_from_tick_object(&tick_obj) {
                Some(ln) => ln,
                None => continue,
            };

            if liquidity_net != 0 {
                ticks.push(TickInfo {
                    tick_index,
                    liquidity_net,
                });
            }
        }

        cursor = page
            .get("nextCursor")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());

        if cursor.is_none() {
            break;
        }
    }

    finalize_ticks(&mut ticks);
    Ok(ticks)
}

/// Magma-style SkipList: DF keys are internal scores; tick index lives in the node payload.
pub async fn collect_ticks_from_skip_list(
    client: &dyn SuiClientTrait,
    skip_list_id: &str,
    current_tick_index: i32,
    window: i32,
) -> Result<Vec<TickInfo>> {
    let mut ticks = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0u32;

    while pages < 10 {
        pages += 1;
        let page = client
            .get_dynamic_fields(skip_list_id, cursor.as_deref(), Some(50))
            .await?;

        let data = page
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        if data.is_empty() {
            break;
        }

        for entry in &data {
            let object_id = entry
                .get("objectId")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if object_id.is_empty() {
                continue;
            }

            let tick_obj = match client.get_object(object_id).await {
                Ok(o) => o,
                Err(e) => {
                    tracing::warn!("Failed to fetch skip-list node {}: {:?}", object_id, e);
                    continue;
                }
            };

            let (tick_index, liquidity_net) = match parse_skip_list_tick_object(&tick_obj) {
                Some(v) => v,
                None => continue,
            };

            if !tick_in_window(tick_index, current_tick_index, window) {
                continue;
            }

            if liquidity_net != 0 {
                ticks.push(TickInfo {
                    tick_index,
                    liquidity_net,
                });
            }
        }

        cursor = page
            .get("nextCursor")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());

        if cursor.is_none() {
            break;
        }
    }

    finalize_ticks(&mut ticks);
    Ok(ticks)
}

/// Momentum-style `Table<I32, TickInfo>`: paginate table dynamic fields.
pub async fn collect_ticks_from_raw_i32_table(
    client: &dyn SuiClientTrait,
    table_id: &str,
    current_tick_index: i32,
    window: i32,
) -> Result<Vec<TickInfo>> {
    let mut ticks = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0u32;

    while pages < 10 {
        pages += 1;
        let page = client
            .get_dynamic_fields(table_id, cursor.as_deref(), Some(50))
            .await?;

        let data = page
            .get("data")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        if data.is_empty() {
            break;
        }

        for entry in &data {
            let name = match entry.get("name") {
                Some(n) => n,
                None => continue,
            };
            let tick_index = match parse_raw_tick_index_from_df_name(name) {
                Some(t) => t,
                None => continue,
            };
            if !tick_in_window(tick_index, current_tick_index, window) {
                continue;
            }

            let object_id = entry
                .get("objectId")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if object_id.is_empty() {
                continue;
            }

            let tick_obj = match client.get_object(object_id).await {
                Ok(o) => o,
                Err(e) => {
                    tracing::warn!("Failed to fetch table tick {}: {:?}", object_id, e);
                    continue;
                }
            };

            let (parsed_index, liquidity_net) = match parse_table_tick_object(&tick_obj) {
                Some(v) => v,
                None => continue,
            };

            if liquidity_net != 0 {
                ticks.push(TickInfo {
                    tick_index: parsed_index,
                    liquidity_net,
                });
            }
        }

        cursor = page
            .get("nextCursor")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());

        if cursor.is_none() {
            break;
        }
    }

    finalize_ticks(&mut ticks);
    Ok(ticks)
}

/// Turbos-style: tick objects are dynamic fields on the pool keyed by raw `I32` tick index.
pub async fn collect_turbos_ticks(
    client: &dyn SuiClientTrait,
    pool_id: &str,
    i32_type: &str,
    current_tick_index: i32,
    tick_spacing: u32,
) -> Result<Vec<TickInfo>> {
    let spacing = tick_spacing as i32;
    let mut ticks = Vec::new();

    for tick_index in aligned_ticks_in_window(current_tick_index, spacing, MAX_TICK_FETCH_WINDOW) {
        let name = json!({
            "type": i32_type,
            "value": { "bits": encode_raw_i32(tick_index) }
        });

        let tick_obj = match client.get_dynamic_field_object(pool_id, &name).await {
            Ok(o) => o,
            Err(_) => continue,
        };

        let liquidity_net = match parse_liquidity_net_from_tick_object(&tick_obj) {
            Some(ln) => ln,
            None => continue,
        };

        if liquidity_net != 0 {
            ticks.push(TickInfo {
                tick_index,
                liquidity_net,
            });
        }
    }

    finalize_ticks(&mut ticks);
    Ok(ticks)
}

pub fn build_pool_tick_data(
    pool_id: &str,
    current_tick_index: i32,
    tick_spacing: u32,
    ticks: Vec<TickInfo>,
    fee_growth_global_a: Option<u128>,
    fee_growth_global_b: Option<u128>,
) -> PoolTickData {
    PoolTickData {
        pool_id: pool_id.to_string(),
        current_tick_index,
        tick_spacing,
        ticks,
        fee_growth_global_a,
        fee_growth_global_b,
    }
}

fn finalize_ticks(ticks: &mut Vec<TickInfo>) {
    ticks.sort_by_key(|t| t.tick_index);
    ticks.dedup_by_key(|t| t.tick_index);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_aligned_ticks_in_window_negative_current() {
        let ticks = aligned_ticks_in_window(-72_294, 60, 200);
        assert!(!ticks.is_empty());
        assert!(ticks.iter().all(|t| *t % 60 == 0));
        assert!(ticks.contains(&-72_300) || ticks.contains(&-72_240));
        for t in &ticks {
            assert!(tick_in_window(*t, -72_294, 200));
        }
    }

    #[test]
    fn test_parse_u128_field_formats() {
        use super::*;
        assert_eq!(parse_u128_value(&json!("12345")).unwrap(), 12_345);
        assert_eq!(
            parse_optional_u128_field(
                &json!({ "fee_growth_global_a": { "fields": { "bits": "99" } } }),
                &["fee_growth_global_a"]
            )
            .unwrap(),
            Some(99)
        );
        assert_eq!(
            parse_optional_u128_field(&json!({}), &["fee_growth_global_a"]).unwrap(),
            None
        );
    }

    #[test]
    fn test_parse_skip_list_tick_object() {
        let obj = json!({
            "data": {
                "content": {
                    "type": "0xpkg::skip_list::Node",
                    "fields": {
                        "value": {
                            "fields": {
                                "value": {
                                    "fields": {
                                        "index": { "fields": { "bits": 61_520 } },
                                        "liquidity_net": { "fields": { "bits": "1445703241" } }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
        let (idx, ln) = parse_skip_list_tick_object(&obj).unwrap();
        assert_eq!(idx, 61_520);
        assert_eq!(ln, -361_425_810);
    }

    #[test]
    fn test_parse_table_tick_object() {
        let obj = json!({
            "data": {
                "content": {
                    "type": "0x2::dynamic_field::Field",
                    "fields": {
                        "name": {
                            "type": "0xpkg::i32::I32",
                            "fields": { "bits": 6_882 }
                        },
                        "value": {
                            "fields": {
                                "liquidity_net": { "fields": { "bits": "89456744268" } }
                            }
                        }
                    }
                }
            }
        });
        let (idx, ln) = parse_table_tick_object(&obj).unwrap();
        assert_eq!(idx, 6_882);
        assert_eq!(ln, 22_364_186_067);
    }
}
