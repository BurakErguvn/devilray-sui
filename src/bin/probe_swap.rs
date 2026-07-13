//! Manual on-chain verification CLI for Magma/Momentum swap constants.
//!
//! Usage:
//!   cargo run --bin probe_swap -- magma <pool_id>
//!   cargo run --bin probe_swap -- momentum <pool_id>
//!   cargo run --bin probe_swap -- verify-config

use devilray_sui::dex_swap::{MAGMA_PACKAGE, MOMENTUM_PACKAGE, MOMENTUM_VERSION};
use devilray_sui::discovery::registry::MAGMA_GLOBAL_CONFIG;
use devilray_sui::sui_client::{SuiClient, SuiClientTrait};
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "probe_swap=info".into()),
        )
        .init();

    let args: Vec<String> = env::args().collect();
    let rpc_url =
        env::var("RPC_URL").unwrap_or_else(|_| "https://fullnode.mainnet.sui.io:443".to_string());
    let client = SuiClient::new(rpc_url, String::new());

    if args.len() >= 2 && args[1] == "verify-config" {
        println!("=== Verified DEX swap constants ===");
        println!("MAGMA_PACKAGE:       {}", MAGMA_PACKAGE);
        println!("MAGMA_GLOBAL_CONFIG: {}", MAGMA_GLOBAL_CONFIG);
        println!("MOMENTUM_PACKAGE:    {}", MOMENTUM_PACKAGE);
        println!("MOMENTUM_VERSION:    {}", MOMENTUM_VERSION);
        println!();
        println!("Probing Magma GlobalConfig object...");
        let cfg = client.get_object(MAGMA_GLOBAL_CONFIG).await?;
        if cfg.get("data").is_some() || cfg.get("object").is_some() {
            println!("  OK: GlobalConfig object exists on-chain");
        } else {
            eprintln!("  WARN: unexpected response format: {:?}", cfg);
        }
        println!("Probing Momentum Version object...");
        let ver = client.get_object(MOMENTUM_VERSION).await?;
        if ver.get("data").is_some() || ver.get("object").is_some() {
            println!("  OK: Version object exists on-chain");
        } else {
            eprintln!("  WARN: unexpected response format: {:?}", ver);
        }
        return Ok(());
    }

    if args.len() < 3 {
        eprintln!("Usage:");
        eprintln!("  cargo run --bin probe_swap -- verify-config");
        eprintln!("  cargo run --bin probe_swap -- magma <pool_id>");
        eprintln!("  cargo run --bin probe_swap -- momentum <pool_id>");
        std::process::exit(1);
    }

    let dex = args[1].to_lowercase();
    let pool_id = &args[2];

    println!("Fetching pool {} for DEX '{}'...", pool_id, dex);
    let response = client.get_object(pool_id).await?;

    let type_str = response
        .pointer("/data/content/type")
        .or_else(|| response.pointer("/object/asMoveObject/type/repr"))
        .and_then(|t| t.as_str())
        .unwrap_or("<unknown>");

    println!("Pool type: {}", type_str);

    let expected_pkg = match dex.as_str() {
        "magma" => MAGMA_PACKAGE,
        "momentum" => MOMENTUM_PACKAGE,
        _ => {
            eprintln!("Unknown dex '{}'. Use magma or momentum.", dex);
            std::process::exit(1);
        }
    };

    if type_str.contains(expected_pkg.trim_start_matches("0x"))
        || type_str.contains(&expected_pkg[2..10])
    {
        println!("OK: pool type references expected package {}", expected_pkg);
    } else {
        println!(
            "WARN: pool type may not match expected package {}",
            expected_pkg
        );
        println!("      Verify module path manually from type string above.");
    }

    Ok(())
}
