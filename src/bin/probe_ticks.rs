//! On-chain tick storage / encoding probe for CLMM DEX collectors.
//!
//! Usage:
//!   cargo run --bin probe_ticks -- <dex> <pool_id> [rpc_url]
//!
//! Senior decisions (Faz 0):
//! - **Cetus**: `current_tick_index` = integer_mate I32; ticks on pool DF (I32 name).
//! - **Turbos**: `tick_current_index` = raw i32 bits; Tick objects on pool via
//!   `suix_getDynamicFieldObject` (I32 key); `tick_map` is bitmap only.
//! - **Magma**: `current_tick_index` = raw i32; ticks in `tick_manager.ticks` SkipList DF.
//! - **Momentum**: `tick_index` = raw i32; ticks in `ticks` Table DF.

use devilray_sui::collectors::tick_fetch::{
    aligned_ticks_in_window, extract_nested_object_id, parse_cetus_tick_index,
    parse_fee_growth_global, parse_move_object_response, parse_raw_tick_index, parse_u32_field,
};
use devilray_sui::sui_client::{SuiClient, SuiClientTrait};
use devilray_sui::sui_int::{decode_raw_i32, decode_sui_i32, parse_i32_bits_from_json};
use std::env;

const MAX_DF_SAMPLE: u32 = 8;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "probe_ticks=info".into()),
        )
        .init();

    let mut args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() < 2 {
        eprintln!("Usage: cargo run --bin probe_ticks -- <dex> <pool_id> [rpc_url]");
        eprintln!("  dex: cetus | turbos | magma | momentum");
        std::process::exit(1);
    }

    let dex = args.remove(0).to_lowercase();
    let pool_id = args.remove(0);
    let rpc_url = args
        .first()
        .cloned()
        .or_else(|| env::var("RPC_URL").ok())
        .unwrap_or_else(|| "https://fullnode.mainnet.sui.io:443".to_string());

    let client = SuiClient::new(rpc_url.clone(), String::new());

    println!("=== probe_ticks: {} ===", dex);
    println!("Pool: {}", pool_id);
    println!("RPC:  {}", rpc_url);
    println!();

    let response = client.get_object(&pool_id).await?;
    let (fields, type_str) = parse_move_object_response(&response)?;

    println!("Pool type: {}", type_str);
    println!("Field keys: {:?}", field_keys(&fields));
    println!();

    print_tick_related_fields(&dex, &fields);
    match parse_fee_growth_global(&fields) {
        Ok((a, b)) => println!("Fee growth (normalized a/b): a={:?} b={:?}", a, b),
        Err(e) => println!("Fee growth parse error: {:?}", e),
    }
    println!();

    let df_page = client
        .get_dynamic_fields(&pool_id, None, Some(MAX_DF_SAMPLE))
        .await?;
    summarize_dynamic_fields("pool", &df_page);

    match dex.as_str() {
        "turbos" => probe_turbos(&client, &pool_id, &fields).await?,
        "magma" => probe_magma(&client, &fields).await?,
        "momentum" => probe_momentum(&client, &fields).await?,
        "cetus" => probe_cetus(&client, &pool_id).await?,
        other => {
            eprintln!(
                "Unknown dex '{}'. Use cetus, turbos, magma, or momentum.",
                other
            );
            std::process::exit(1);
        }
    }

    println!();
    println!("=== Senior gate summary ===");
    match dex.as_str() {
        "cetus" => {
            println!("Encoding: integer_mate I32 (decode_sui_i32)");
            println!("Storage: pool dynamic fields, I32 tick name");
        }
        "turbos" => {
            println!("Encoding: raw i32 bits (decode_raw_i32)");
            println!("Storage: pool dynamic_field_object per aligned tick; tick_map=bitmap");
        }
        "magma" => {
            println!("Encoding: raw i32 current tick; integer_mate I128 liquidity_net");
            println!("Storage: tick_manager.ticks SkipList dynamic fields");
        }
        "momentum" => {
            println!("Encoding: raw i32 tick_index; integer_mate I128 liquidity_net");
            println!("Storage: ticks Table dynamic fields (I32 key)");
        }
        _ => {}
    }

    Ok(())
}

fn field_keys(fields: &serde_json::Value) -> Vec<String> {
    fields
        .as_object()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default()
}

fn print_tick_related_fields(dex: &str, fields: &serde_json::Value) {
    let highlights: &[&str] = match dex {
        "turbos" => &[
            "tick_current_index",
            "tick_spacing",
            "tick_map",
            "sqrt_price",
            "fee_growth_global_a",
            "fee_growth_global_b",
        ],
        "magma" => &[
            "current_tick_index",
            "tick_spacing",
            "tick_manager",
            "current_sqrt_price",
            "fee_growth_global_a",
            "fee_growth_global_b",
        ],
        "momentum" => &[
            "tick_index",
            "tick_spacing",
            "ticks",
            "tick_bitmap",
            "sqrt_price",
            "fee_growth_global_x",
            "fee_growth_global_y",
        ],
        _ => &[
            "current_tick_index",
            "tick_spacing",
            "ticks_manager",
            "current_sqrt_price",
            "fee_growth_global_a",
            "fee_growth_global_b",
        ],
    };

    for key in highlights {
        if let Some(val) = fields.get(*key) {
            let snippet = serde_json::to_string(val).unwrap_or_default();
            let trimmed = if snippet.len() > 220 {
                format!("{}…", &snippet[..220])
            } else {
                snippet
            };
            println!("  {} = {}", key, trimmed);
        }
    }

    if let Ok(current) = parse_raw_tick_index(fields, &["tick_current_index", "tick_index"]) {
        if let Ok(spacing) = parse_u32_field(fields, &["tick_spacing"]) {
            let aligned = aligned_ticks_in_window(current, spacing as i32, 200);
            println!(
                "  cross-check raw tick={} spacing={} aligned_in_window={}",
                current,
                spacing,
                aligned.len()
            );
        }
    } else if let Ok(current) = parse_cetus_tick_index(fields) {
        println!("  cross-check integer_mate tick={}", current);
    }
}

fn summarize_dynamic_fields(label: &str, page: &serde_json::Value) {
    let data = page
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    println!("{} dynamic fields (first {}):", label, data.len());
    for entry in &data {
        let name_type = entry
            .pointer("/name/type")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let name_val = entry.get("name").map(|n| {
            if let Some(bits) = parse_i32_bits_from_json(n.get("value").unwrap_or(n)) {
                format!("im={} raw={}", decode_sui_i32(bits), decode_raw_i32(bits))
            } else if let Some(v) = n.get("value").and_then(|x| x.as_str()) {
                v.chars().take(40).collect::<String>()
            } else {
                "?".to_string()
            }
        });
        let object_type = entry
            .get("objectType")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let object_id = entry
            .get("objectId")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        println!(
            "  - name_type={} name_val={:?} object_type={} object_id={}",
            name_type, name_val, object_type, object_id
        );
    }
}

async fn probe_cetus(
    client: &dyn SuiClientTrait,
    pool_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    summarize_dynamic_fields(
        "cetus pool",
        &client
            .get_dynamic_fields(pool_id, None, Some(MAX_DF_SAMPLE))
            .await?,
    );
    Ok(())
}

async fn probe_turbos(
    client: &dyn SuiClientTrait,
    pool_id: &str,
    fields: &serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(tick_map) = fields.get("tick_map")
        && let Some(table_id) = extract_nested_object_id(tick_map)
    {
        println!("tick_map table id: {}", table_id);
        let page = client
            .get_dynamic_fields(&table_id, None, Some(MAX_DF_SAMPLE))
            .await?;
        summarize_dynamic_fields("tick_map (bitmap words)", &page);
    }

    if let (Ok(current), Ok(spacing)) = (
        parse_raw_tick_index(fields, &["tick_current_index"]),
        parse_u32_field(fields, &["tick_spacing"]),
    ) {
        let i32_type = fields
            .get("tick_current_index")
            .and_then(|v| v.get("type"))
            .and_then(|t| t.as_str())
            .unwrap_or("<unknown>");
        println!("Turbos I32 type: {}", i32_type);
        if let Some(sample_tick) = aligned_ticks_in_window(current, spacing as i32, 200).first() {
            let name = serde_json::json!({
                "type": i32_type,
                "value": { "bits": devilray_sui::sui_int::encode_raw_i32(*sample_tick) }
            });
            match client.get_dynamic_field_object(pool_id, &name).await {
                Ok(obj) => {
                    let ln = obj
                        .pointer("/data/content/fields/value/fields/liquidity_net")
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "<missing>".to_string());
                    println!(
                        "sample tick {} dynamic_field_object liquidity_net: {}",
                        sample_tick, ln
                    );
                }
                Err(e) => println!("sample tick {} not found: {:?}", sample_tick, e),
            }
        }
    }
    Ok(())
}

async fn probe_magma(
    client: &dyn SuiClientTrait,
    fields: &serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(tm) = fields.get("tick_manager")
        && let Some(ticks) = tm.pointer("fields/ticks")
        && let Some(id) = extract_nested_object_id(ticks)
    {
        println!("tick_manager.ticks SkipList id: {}", id);
        let page = client
            .get_dynamic_fields(&id, None, Some(MAX_DF_SAMPLE))
            .await?;
        summarize_dynamic_fields("magma skip list", &page);
    }
    Ok(())
}

async fn probe_momentum(
    client: &dyn SuiClientTrait,
    fields: &serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(ticks) = fields.get("ticks")
        && let Some(id) = extract_nested_object_id(ticks)
    {
        println!("ticks table id: {}", id);
        let page = client
            .get_dynamic_fields(&id, None, Some(MAX_DF_SAMPLE))
            .await?;
        summarize_dynamic_fields("momentum ticks table", &page);
    }
    if let Some(bitmap) = fields.get("tick_bitmap")
        && let Some(id) = extract_nested_object_id(bitmap)
    {
        println!("tick_bitmap table id: {}", id);
    }
    Ok(())
}
