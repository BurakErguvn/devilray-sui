//! GraphQL query builders for paginated pool discovery.

pub const DISCOVER_OBJECTS_QUERY: &str = r#"
query DiscoverPoolObjects($first: Int!, $after: String, $type: String!) {
  objects(first: $first, after: $after, filter: { type: $type }) {
    pageInfo {
      hasNextPage
      endCursor
    }
    nodes {
      address
      version
      digest
      asMoveObject {
        contents {
          type {
            repr
          }
          json
        }
      }
    }
  }
}
"#;

pub const DISCOVER_EVENTS_QUERY: &str = r#"
query DiscoverPoolEvents($first: Int!, $after: String, $type: String!, $afterCheckpoint: UInt53) {
  events(
    first: $first
    after: $after
    filter: { type: $type, afterCheckpoint: $afterCheckpoint }
  ) {
    pageInfo {
      hasNextPage
      endCursor
    }
    nodes {
      sequenceNumber
      timestamp
      contents {
        type {
          repr
        }
        json
      }
      transaction {
        digest
        effects {
          checkpoint {
            sequenceNumber
          }
        }
      }
    }
  }
}
"#;

pub const SERVICE_CONFIG_RANGE_QUERY: &str = r#"
query ServiceConfigRange {
  serviceConfig {
    eventsAvailableRange: availableRange(type: "Query", field: "events") {
      first {
        sequenceNumber
      }
      last {
        sequenceNumber
      }
    }
  }
}
"#;

pub fn discover_objects_variables(
    pool_type: &str,
    first: u32,
    after: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "first": first,
        "after": after,
        "type": pool_type
    })
}

pub fn discover_events_variables(
    event_type: &str,
    first: u32,
    after: Option<&str>,
    after_checkpoint: Option<u64>,
) -> serde_json::Value {
    serde_json::json!({
        "first": first,
        "after": after,
        "type": event_type,
        "afterCheckpoint": after_checkpoint
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discover_objects_variables_shape() {
        let v = discover_objects_variables("0xpkg::pool::Pool", 25, Some("cursor_1"));
        assert_eq!(v["first"], 25);
        assert_eq!(v["type"], "0xpkg::pool::Pool");
    }

    #[test]
    fn test_discover_events_variables_shape() {
        let v = discover_events_variables(
            "0xpkg::factory::CreatePoolEvent",
            50,
            Some("cursor_1"),
            Some(100),
        );
        assert_eq!(v["first"], 50);
        assert_eq!(v["after"], "cursor_1");
        assert_eq!(v["type"], "0xpkg::factory::CreatePoolEvent");
        assert_eq!(v["afterCheckpoint"], 100);
    }
}
