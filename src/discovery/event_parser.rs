//! Normalize GraphQL event nodes into lightweight pool references.

use crate::discovery::registry::DexDiscoverySpec;
use anyhow::{Result, anyhow};
use serde_json::Value;

/// Lightweight pool reference from a create-pool event (hydrate via `fetch_pool`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryPoolRef {
    pub pool_id: String,
    pub dex_name: String,
    pub event_id: String,
    pub checkpoint: Option<u64>,
}

/// Parsed GraphQL event page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryEventRef {
    pub event_id: String,
    pub dex_name: String,
    pub pool_ref: Option<DiscoveryPoolRef>,
    pub parse_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryPage {
    pub events: Vec<DiscoveryEventRef>,
    pub has_next_page: bool,
    pub end_cursor: Option<String>,
}

/// Extract event JSON + type repr from flat or `contents`-nested GraphQL nodes.
pub fn event_type_repr(node: &Value) -> Option<&str> {
    node.pointer("/type/repr")
        .or_else(|| node.pointer("/contents/type/repr"))
        .and_then(|v| v.as_str())
}

pub fn event_json_fields(node: &Value) -> Option<&Value> {
    node.get("json").or_else(|| node.pointer("/contents/json"))
}

pub fn event_id(node: &Value) -> String {
    if let Some(id) = node.get("id").and_then(|v| v.as_str()) {
        return id.to_string();
    }
    let digest = node.pointer("/transaction/digest").and_then(|v| v.as_str());
    let seq = node.get("sequenceNumber").and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    });
    match (digest, seq) {
        (Some(d), Some(s)) => format!("{d}:{s}"),
        (None, Some(s)) => format!("seq:{s}"),
        _ => "unknown".to_string(),
    }
}

pub fn event_checkpoint(node: &Value) -> Option<u64> {
    node.pointer("/transaction/effects/checkpoint/sequenceNumber")
        .or_else(|| node.pointer("/checkpoint/sequenceNumber"))
        .or_else(|| node.get("checkpoint"))
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
}

/// Parse a GraphQL `events.nodes[]` entry — only `pool_id + dex + checkpoint` required.
pub fn parse_create_pool_event(
    node: &Value,
    specs: &[DexDiscoverySpec],
) -> Option<DiscoveryPoolRef> {
    let type_repr = event_type_repr(node)?;
    let type_lower = type_repr.to_lowercase();

    let spec = specs.iter().find(|s| {
        s.create_pool_event_types
            .iter()
            .any(|t| type_lower.contains(&t.to_lowercase()))
            || type_lower.contains(&s.package_id.to_lowercase())
                && (type_lower.contains("createpoolevent")
                    || type_lower.contains("poolcreatedevent"))
    })?;

    if !type_lower.contains("createpoolevent") && !type_lower.contains("poolcreatedevent") {
        return None;
    }

    let json_fields = event_json_fields(node)?;
    let pool_id = json_fields
        .get("pool_id")
        .or_else(|| json_fields.get("pool"))
        .or_else(|| json_fields.get("id"))
        .and_then(|v| v.as_str())?
        .to_string();

    Some(DiscoveryPoolRef {
        pool_id,
        dex_name: spec.dex_name.to_string(),
        event_id: event_id(node),
        checkpoint: event_checkpoint(node),
    })
}

pub fn parse_event_page(data: &Value, specs: &[DexDiscoverySpec]) -> Result<DiscoveryPage> {
    let events_val = data
        .get("events")
        .ok_or_else(|| anyhow!("missing events in GraphQL data"))?;
    let nodes = events_val
        .get("nodes")
        .and_then(|n| n.as_array())
        .cloned()
        .unwrap_or_default();
    let page_info = events_val.get("pageInfo");
    let has_next_page = page_info
        .and_then(|p| p.get("hasNextPage"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let end_cursor = page_info
        .and_then(|p| p.get("endCursor"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut events = Vec::with_capacity(nodes.len());
    for node in nodes {
        let eid = event_id(&node);
        match parse_create_pool_event(&node, specs) {
            Some(pool_ref) => events.push(DiscoveryEventRef {
                event_id: eid,
                dex_name: pool_ref.dex_name.clone(),
                pool_ref: Some(pool_ref),
                parse_error: None,
            }),
            None => {
                if event_type_repr(&node).is_some() {
                    continue;
                }
                events.push(DiscoveryEventRef {
                    event_id: eid,
                    dex_name: String::new(),
                    pool_ref: None,
                    parse_error: Some("unrecognized event node shape".to_string()),
                });
            }
        }
    }

    Ok(DiscoveryPage {
        events,
        has_next_page,
        end_cursor,
    })
}

/// Validates normalized Move event struct fields against the locked registry spec.
pub fn validate_event_contract_fields(
    spec: &DexDiscoverySpec,
    struct_field_names: &[&str],
) -> Result<(), String> {
    for field in spec.required_event_fields {
        if !struct_field_names.iter().any(|f| f == field) {
            return Err(format!(
                "missing required event field `{field}` for {}",
                spec.dex_name
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::fixtures;
    use crate::discovery::registry::ALL_DISCOVERY_SPECS;

    #[test]
    fn test_parse_cetus_fixture_node() {
        let node = fixtures::cetus_create_pool_event_node();
        let meta = parse_create_pool_event(&node, ALL_DISCOVERY_SPECS).unwrap();
        assert_eq!(meta.dex_name, "Cetus");
        assert_eq!(meta.pool_id, fixtures::CETUS_POOL_ID);
    }

    #[test]
    fn test_parse_all_dex_fixtures() {
        for (node, dex) in [
            (fixtures::cetus_create_pool_event_node(), "Cetus"),
            (fixtures::turbos_create_pool_event_node(), "Turbos"),
            (fixtures::magma_create_pool_event_node(), "Magma"),
            (fixtures::momentum_create_pool_event_node(), "Momentum"),
        ] {
            let meta = parse_create_pool_event(&node, ALL_DISCOVERY_SPECS).unwrap();
            assert_eq!(meta.dex_name, dex);
        }
    }

    #[test]
    fn test_momentum_type_x_y_event() {
        let node = serde_json::json!({
            "sequenceNumber": 1,
            "transaction": { "digest": "0xmom" },
            "contents": {
                "type": { "repr": "0x70285592c97965e811e0c6f98dccc3a9c2b4ad854b3594faab9597ada267b860::create_pool::PoolCreatedEvent" },
                "json": {
                    "pool_id": "0x_momentum_pool",
                    "type_x": { "name": "0x2::sui::SUI" },
                    "type_y": { "name": "0xusdc::coin::COIN" },
                    "fee_rate": "2000"
                }
            }
        });
        let meta = parse_create_pool_event(&node, ALL_DISCOVERY_SPECS).unwrap();
        assert_eq!(meta.pool_id, "0x_momentum_pool");
    }

    #[test]
    fn test_parse_event_page_two_pages() {
        let data = fixtures::two_page_graphql_response();
        let page = parse_event_page(&data, ALL_DISCOVERY_SPECS).unwrap();
        assert_eq!(page.events.len(), 1);
        assert!(page.has_next_page);
        assert_eq!(page.end_cursor.as_deref(), Some("cursor_page_2"));
    }

    #[test]
    fn test_event_id_from_transaction_digest() {
        let node = serde_json::json!({
            "sequenceNumber": 2,
            "transaction": { "digest": "0xabc123" }
        });
        assert_eq!(event_id(&node), "0xabc123:2");
    }
}
