//! Parse GraphQL pool object nodes into discovery references.

use crate::discovery::registry::DexDiscoverySpec;
use anyhow::{Result, anyhow};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryObjectRef {
    pub pool_id: String,
    pub dex_name: String,
    pub type_repr: String,
    pub parse_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryObjectPage {
    pub objects: Vec<DiscoveryObjectRef>,
    pub has_next_page: bool,
    pub end_cursor: Option<String>,
}

pub fn object_type_repr(node: &Value) -> Option<&str> {
    node.pointer("/asMoveObject/contents/type/repr")
        .or_else(|| node.pointer("/contents/type/repr"))
        .and_then(|v| v.as_str())
}

pub fn object_json_fields(node: &Value) -> Option<&Value> {
    node.pointer("/asMoveObject/contents/json")
        .or_else(|| node.pointer("/contents/json"))
        .or_else(|| node.get("json"))
}

pub fn parse_object_page(data: &Value, specs: &[DexDiscoverySpec]) -> Result<DiscoveryObjectPage> {
    let objects_val = data
        .get("objects")
        .ok_or_else(|| anyhow!("missing objects in GraphQL data"))?;
    let nodes = objects_val
        .get("nodes")
        .and_then(|n| n.as_array())
        .cloned()
        .unwrap_or_default();
    let page_info = objects_val.get("pageInfo");
    let has_next_page = page_info
        .and_then(|p| p.get("hasNextPage"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let end_cursor = page_info
        .and_then(|p| p.get("endCursor"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let mut objects = Vec::with_capacity(nodes.len());
    for node in nodes {
        let pool_id = node
            .get("address")
            .and_then(|v| v.as_str())
            .or_else(|| {
                object_json_fields(&node)
                    .and_then(|j| j.get("id"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or_default()
            .to_string();

        let type_repr = object_type_repr(&node).unwrap_or_default();
        if pool_id.is_empty() {
            objects.push(DiscoveryObjectRef {
                pool_id,
                dex_name: String::new(),
                type_repr: type_repr.to_string(),
                parse_error: Some("missing pool object address".to_string()),
            });
            continue;
        }

        let spec = specs.iter().find(|s| {
            type_repr
                .to_lowercase()
                .contains(&s.package_id.to_lowercase())
                && type_repr.contains(&format!("::{}::Pool", s.pool_type_module))
        });

        match spec {
            Some(s) => objects.push(DiscoveryObjectRef {
                pool_id,
                dex_name: s.dex_name.to_string(),
                type_repr: type_repr.to_string(),
                parse_error: None,
            }),
            None => objects.push(DiscoveryObjectRef {
                pool_id,
                dex_name: String::new(),
                type_repr: type_repr.to_string(),
                parse_error: Some("unrecognized pool object type".to_string()),
            }),
        }
    }

    Ok(DiscoveryObjectPage {
        objects,
        has_next_page,
        end_cursor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::fixtures;
    use crate::discovery::registry::ALL_DISCOVERY_SPECS;

    #[test]
    fn test_parse_cetus_object_fixture() {
        let data = fixtures::cetus_object_graphql_response();
        let page = parse_object_page(&data, ALL_DISCOVERY_SPECS).unwrap();
        assert_eq!(page.objects.len(), 1);
        assert_eq!(page.objects[0].dex_name, "Cetus");
        assert_eq!(page.objects[0].pool_id, fixtures::CETUS_POOL_ID);
    }

    #[test]
    fn test_parse_all_dex_object_fixtures() {
        for (data, dex) in [
            (fixtures::cetus_object_graphql_response(), "Cetus"),
            (fixtures::turbos_object_graphql_response(), "Turbos"),
            (fixtures::magma_object_graphql_response(), "Magma"),
            (fixtures::momentum_object_graphql_response(), "Momentum"),
        ] {
            let page = parse_object_page(&data, ALL_DISCOVERY_SPECS).unwrap();
            assert_eq!(page.objects[0].dex_name, dex);
        }
    }

    #[test]
    fn test_empty_object_page() {
        let data = serde_json::json!({
            "objects": {
                "pageInfo": { "hasNextPage": false, "endCursor": null },
                "nodes": []
            }
        });
        let page = parse_object_page(&data, ALL_DISCOVERY_SPECS).unwrap();
        assert!(page.objects.is_empty());
        assert!(!page.has_next_page);
    }
}
