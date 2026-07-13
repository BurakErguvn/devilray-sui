use devilray_sui::collectors::DexDataCollector;
use devilray_sui::collectors::cetus::CetusCollector;
use devilray_sui::collectors::magma::MagmaCollector;
use devilray_sui::collectors::momentum::MomentumCollector;
use devilray_sui::collectors::turbos::TurbosCollector;
use devilray_sui::sui_client::SuiClient;
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        println!(
            "Usage: cargo run --bin fetch_pool -- <dex> <pool_id> [rpc_url] [graphql_url] [decimals_a] [decimals_b] [factor_bits]"
        );
        println!("\nExample Cetus:");
        println!(
            "  cargo run --bin fetch_pool -- cetus 0xcf994611fd4c48e277ce3ffd4d4364c914af2c3cbb05f7bf6facd371de688630"
        );
        return Ok(());
    }

    let dex_name = args[1].to_lowercase();
    let pool_id = &args[2];

    let rpc_url = args
        .get(3)
        .cloned()
        .unwrap_or_else(|| "https://fullnode.mainnet.sui.io:443".to_string());
    let graphql_url = args
        .get(4)
        .cloned()
        .unwrap_or_else(|| "https://graphql.mainnet.sui.io/graphql".to_string());

    let decimals_a: u8 = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(9); // SUI default decimals = 9
    let decimals_b: u8 = args.get(6).and_then(|s| s.parse().ok()).unwrap_or(6); // USDC default decimals = 6

    let default_factor_bits = if dex_name == "turbos" { 96 } else { 64 };
    let factor_bits: u32 = args
        .get(7)
        .and_then(|s| s.parse().ok())
        .unwrap_or(default_factor_bits);

    println!("--------------------------------------------------");
    println!("DevilRay - Pool Fetcher Verification Tool");
    println!("--------------------------------------------------");
    println!("DEX:           {}", dex_name);
    println!("Pool ID:       {}", pool_id);
    println!("RPC URL:       {}", rpc_url);
    println!("GraphQL URL:   {}", graphql_url);
    println!("Decimals A:    {}", decimals_a);
    println!("Decimals B:    {}", decimals_b);
    println!("Factor Bits:   {}", factor_bits);
    println!("--------------------------------------------------");

    let client = SuiClient::new(rpc_url, graphql_url);

    let collector: Box<dyn DexDataCollector> = match dex_name.as_str() {
        "cetus" => Box::new(CetusCollector::new()),
        "turbos" => Box::new(TurbosCollector::new()),
        "magma" => Box::new(MagmaCollector::new()),
        "momentum" => Box::new(MomentumCollector::new()),
        _ => {
            println!(
                "Error: Unsupported DEX '{}'. Supported: cetus, turbos, magma, momentum",
                dex_name
            );
            return Ok(());
        }
    };

    println!("Fetching pool state...");
    match collector.fetch_pool(&client, pool_id).await {
        Ok(pool_state) => {
            println!("Successfully fetched pool!");
            println!("{:#?}", pool_state);

            let price = pool_state.calculate_price(decimals_a, decimals_b, factor_bits);
            println!("--------------------------------------------------");
            println!(
                "Computed human-readable price: {} (Coin B per Coin A)",
                price
            );
            println!("--------------------------------------------------");
        }
        Err(err) => {
            println!("Failed to fetch pool: {:?}", err);
        }
    }

    Ok(())
}
