//! Discovery checkpoint and failure persistence models (source-aware).

use serde::{Deserialize, Serialize};

/// Discovery scan source: authoritative object bootstrap vs incremental events.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySource {
    ObjectBootstrap,
    EventIncremental,
}

impl DiscoverySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ObjectBootstrap => "object_bootstrap",
            Self::EventIncremental => "event_incremental",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PoolDiscoveryProgress {
    pub dex_name: String,
    /// Progress key: `object_bootstrap` or fully-qualified event type.
    pub source_key: String,
    pub discovery_source: DiscoverySource,
    pub last_checkpoint: Option<u64>,
    pub page_cursor: Option<String>,
    /// Event incremental backfill reached the retention window end.
    pub backfill_complete: bool,
    /// Object bootstrap consumed all pages at least once.
    pub bootstrap_complete: bool,
    /// Event scan could not reach genesis due to GraphQL retention limits.
    pub retention_limited: bool,
    /// Object scan generation; incremented when cursor resets to `None`.
    pub generation: u64,
    pub pools_discovered: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PoolDiscoveryFailure {
    pub dex_name: String,
    pub event_id: String,
    pub pool_id: Option<String>,
    pub reason: String,
    pub attempts: u32,
}

impl PoolDiscoveryProgress {
    pub fn new_object_bootstrap(dex_name: &str) -> Self {
        Self {
            dex_name: dex_name.to_string(),
            source_key: crate::discovery::registry::OBJECT_BOOTSTRAP_SOURCE_KEY.to_string(),
            discovery_source: DiscoverySource::ObjectBootstrap,
            last_checkpoint: None,
            page_cursor: None,
            backfill_complete: false,
            bootstrap_complete: false,
            retention_limited: false,
            generation: 0,
            pools_discovered: 0,
        }
    }

    pub fn new_event_incremental(dex_name: &str, event_type: &str) -> Self {
        Self {
            dex_name: dex_name.to_string(),
            source_key: event_type.to_string(),
            discovery_source: DiscoverySource::EventIncremental,
            last_checkpoint: None,
            page_cursor: None,
            backfill_complete: false,
            bootstrap_complete: false,
            retention_limited: false,
            generation: 0,
            pools_discovered: 0,
        }
    }
}

/// Atomic page commit payload: pools, tokens, progress and failure bookkeeping.
#[derive(Debug, Clone)]
pub struct DiscoveryPageCommit {
    pub progress: PoolDiscoveryProgress,
    pub pools: Vec<crate::models::PoolState>,
    pub tokens: Vec<crate::models::Token>,
    pub failures: Vec<PoolDiscoveryFailure>,
    pub resolved_failure_ids: Vec<String>,
}

impl Default for DiscoveryPageCommit {
    fn default() -> Self {
        Self {
            progress: PoolDiscoveryProgress::new_object_bootstrap("unknown"),
            pools: Vec::new(),
            tokens: Vec::new(),
            failures: Vec::new(),
            resolved_failure_ids: Vec::new(),
        }
    }
}
