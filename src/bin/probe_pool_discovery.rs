//! Probe mainnet pool discovery contracts, object bootstrap and collector hydration.
//!
//! Usage:
//!   cargo run --bin probe_pool_discovery -- range
//!   cargo run --bin probe_pool_discovery -- dex Cetus
//!   cargo run --bin probe_pool_discovery -- all
//!   cargo run --bin probe_pool_discovery -- verify --strict

use devilray_sui::collectors::collector_for_dex_name;
use devilray_sui::discovery::{
    ALL_DISCOVERY_SPECS, DISCOVER_EVENTS_QUERY, DISCOVER_OBJECTS_QUERY, discover_events_variables,
    discover_objects_variables, fetch_graphql_available_range, parse_event_page, parse_object_page,
    validate_event_contract_fields,
};
use devilray_sui::sui_client::{SuiClient, SuiClientTrait};
use std::env;
use std::process;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "probe_pool_discovery=info".into()),
        )
        .init();

    let args: Vec<String> = env::args().collect();
    let graphql_url = env::var("GRAPHQL_URL")
        .unwrap_or_else(|_| "https://graphql.mainnet.sui.io/graphql".to_string());
    let rpc_url =
        env::var("RPC_URL").unwrap_or_else(|_| "https://fullnode.mainnet.sui.io:443".to_string());
    let client = SuiClient::new(rpc_url, graphql_url);

    if args.len() < 2 {
        print_usage();
        process::exit(1);
    }

    match args[1].as_str() {
        "range" => {
            let range = fetch_graphql_available_range(&client).await?;
            println!("GraphQL availableRange:");
            println!("  first checkpoint: {:?}", range.first_checkpoint);
            println!("  last checkpoint:  {:?}", range.last_checkpoint);
        }
        "all" => {
            for spec in ALL_DISCOVERY_SPECS {
                probe_dex(&client, spec.dex_name, false).await?;
            }
        }
        "dex" => {
            let name = args.get(2).map(|s| s.as_str()).unwrap_or("Cetus");
            probe_dex(&client, name, false).await?;
        }
        "verify" => {
            let strict = args.iter().any(|a| a == "--strict");
            let ok = verify_all(&client, strict).await?;
            process::exit(if ok { 0 } else { 1 });
        }
        other => {
            eprintln!("Unknown command: {}", other);
            print_usage();
            process::exit(1);
        }
    }

    Ok(())
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  cargo run --bin probe_pool_discovery -- range");
    eprintln!("  cargo run --bin probe_pool_discovery -- dex <Cetus|Turbos|Magma|Momentum>");
    eprintln!("  cargo run --bin probe_pool_discovery -- all");
    eprintln!("  cargo run --bin probe_pool_discovery -- verify [--strict]");
}

async fn probe_dex(
    client: &dyn SuiClientTrait,
    dex_name: &str,
    _strict: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let spec = ALL_DISCOVERY_SPECS
        .iter()
        .find(|s| s.dex_name.eq_ignore_ascii_case(dex_name))
        .ok_or_else(|| format!("unknown DEX: {}", dex_name))?;

    println!("=== {} ===", spec.dex_name);
    println!("package: {}", spec.package_id);
    println!("pool_object_type: {}", spec.pool_object_type);

    let obj_vars = discover_objects_variables(spec.pool_object_type, 3, None);
    let obj_data = client
        .query_graphql_with_variables(DISCOVER_OBJECTS_QUERY, obj_vars)
        .await?;
    let obj_page = parse_object_page(&obj_data, ALL_DISCOVERY_SPECS)?;
    println!(
        "  object sample: {}, has_next_page: {}",
        obj_page.objects.len(),
        obj_page.has_next_page
    );
    if let Some(obj) = obj_page.objects.first() {
        println!("    pool_id={}", obj.pool_id);
    }

    for event_type in spec.create_pool_event_types {
        println!("event type: {}", event_type);
        let variables = discover_events_variables(event_type, 3, None, None);
        let data = client
            .query_graphql_with_variables(DISCOVER_EVENTS_QUERY, variables)
            .await?;
        let page = parse_event_page(&data, ALL_DISCOVERY_SPECS)?;
        println!(
            "  sample events: {}, has_next_page: {}",
            page.events.len(),
            page.has_next_page
        );
        for ev in page.events.iter().take(2) {
            if let Some(meta) = &ev.pool_ref {
                println!("    pool_id={}", meta.pool_id);
            } else if let Some(err) = &ev.parse_error {
                println!("    parse_error: {}", err);
            }
        }
    }
    Ok(())
}

async fn verify_all(
    client: &dyn SuiClientTrait,
    strict: bool,
) -> Result<bool, Box<dyn std::error::Error>> {
    let range = fetch_graphql_available_range(client).await?;
    println!("verify mode (strict={strict})");
    println!(
        "  event retention: {:?} -> {:?}",
        range.first_checkpoint, range.last_checkpoint
    );

    let mut all_ok = true;

    for spec in ALL_DISCOVERY_SPECS {
        let mut dex_ok = true;
        println!("--- {} ---", spec.dex_name);

        if strict {
            match verify_event_contract(client, spec).await {
                Ok(()) => println!("  contract: OK"),
                Err(err) => {
                    println!("  contract: FAIL ({err})");
                    dex_ok = false;
                }
            }
        }

        let obj_vars = discover_objects_variables(spec.pool_object_type, 5, None);
        let obj_data = client
            .query_graphql_with_variables(DISCOVER_OBJECTS_QUERY, obj_vars)
            .await?;
        let obj_page = parse_object_page(&obj_data, ALL_DISCOVERY_SPECS)?;
        println!("  object_count: {}", obj_page.objects.len());
        if strict && obj_page.objects.is_empty() {
            println!("  object scan: FAIL (zero pools)");
            dex_ok = false;
        }

        let mut hydrated = false;
        for obj in obj_page.objects.iter().take(5) {
            if obj.parse_error.is_some() {
                continue;
            }
            let Some(collector) = collector_for_dex_name(&obj.dex_name) else {
                println!("  collector: missing for {}", obj.dex_name);
                dex_ok = false;
                continue;
            };
            match collector.fetch_pool(client, &obj.pool_id).await {
                Ok(pool) => {
                    if pool.liquidity > 0 && pool.fee_rate > 0 && !pool.coin_type_a.is_empty() {
                        hydrated = true;
                        println!(
                            "  hydrate: OK pool={} liq={} fee={}",
                            pool.pool_id, pool.liquidity, pool.fee_rate
                        );
                        break;
                    }
                    println!(
                        "  hydrate: skip pool={} (liq={} fee={})",
                        pool.pool_id, pool.liquidity, pool.fee_rate
                    );
                }
                Err(err) => {
                    println!("  hydrate: FAIL pool={} err={err}", obj.pool_id);
                    if strict {
                        dex_ok = false;
                    }
                }
            }
        }
        if strict && !hydrated {
            println!("  hydrate: FAIL (no nonzero-liquidity pool)");
            dex_ok = false;
        }

        let mut event_samples = 0u32;
        let mut event_backfill_complete = true;
        for event_type in spec.create_pool_event_types {
            let variables = discover_events_variables(event_type, 1, None, None);
            let data = client
                .query_graphql_with_variables(DISCOVER_EVENTS_QUERY, variables)
                .await?;
            let page = parse_event_page(&data, ALL_DISCOVERY_SPECS)?;
            event_samples += page.events.len() as u32;
            if page.events.is_empty() && range.first_checkpoint.is_some() {
                event_backfill_complete = false;
            }
        }
        println!("  event_samples: {event_samples}");
        println!(
            "  event_backfill_complete: {event_backfill_complete} (retention_limited={})",
            !event_backfill_complete
        );

        if !dex_ok {
            all_ok = false;
        }
    }

    if all_ok {
        println!("VERIFY: PASS");
    } else {
        println!("VERIFY: FAIL");
    }
    Ok(all_ok)
}

async fn verify_event_contract(
    client: &dyn SuiClientTrait,
    spec: &devilray_sui::discovery::DexDiscoverySpec,
) -> Result<(), String> {
    let modules = client
        .get_normalized_move_modules(spec.package_id)
        .await
        .map_err(|e| e.to_string())?;

    for event_type in spec.create_pool_event_types {
        let parts: Vec<&str> = event_type.split("::").collect();
        if parts.len() < 3 {
            return Err(format!("invalid event type: {event_type}"));
        }
        let (module, struct_name) = (parts[1], parts[2]);
        let module_def = modules
            .get(module)
            .ok_or_else(|| format!("missing module {module} in package {}", spec.package_id))?;
        let fields = module_def
            .get("structs")
            .and_then(|s| s.get(struct_name))
            .and_then(|st| st.get("fields"))
            .and_then(|f| f.as_array())
            .ok_or_else(|| format!("missing event struct {struct_name} in module {module}"))?;
        let names: Vec<&str> = fields
            .iter()
            .filter_map(|f| f.get("name").and_then(|n| n.as_str()))
            .collect();
        validate_event_contract_fields(spec, &names)?;
    }
    Ok(())
}
