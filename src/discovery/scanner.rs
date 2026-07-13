//! Paginated GraphQL pool discovery scanner (object bootstrap + event incremental).

use crate::api::metrics;
use crate::discovery::event_parser::{DiscoveryEventRef, parse_event_page};
use crate::discovery::graphql::{
    DISCOVER_EVENTS_QUERY, DISCOVER_OBJECTS_QUERY, SERVICE_CONFIG_RANGE_QUERY,
    discover_events_variables, discover_objects_variables,
};
use crate::discovery::object_parser::{DiscoveryObjectRef, parse_object_page};
use crate::discovery::progress::{
    DiscoveryPageCommit, DiscoverySource, PoolDiscoveryFailure, PoolDiscoveryProgress,
};
use crate::discovery::registry::{
    ALL_DISCOVERY_SPECS, DexDiscoverySpec, OBJECT_BOOTSTRAP_SOURCE_KEY,
};
use crate::storage::PostgresStorageTrait;
use crate::sui_client::SuiClientTrait;
use crate::workers::StaticWorkerConfig;
use anyhow::{Result, anyhow};
use std::collections::HashSet;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct DiscoveryScanStats {
    pub pages_scanned: u32,
    pub objects_seen: u32,
    pub events_seen: u32,
    pub pools_registered: u32,
    pub pools_reconciled: u32,
    pub failures_recorded: u32,
}

#[derive(Debug, Clone)]
pub struct GraphqlAvailableRange {
    pub first_checkpoint: Option<u64>,
    pub last_checkpoint: Option<u64>,
}

pub async fn fetch_graphql_available_range(
    client: &dyn SuiClientTrait,
) -> Result<GraphqlAvailableRange> {
    let data = client
        .query_graphql_with_variables(SERVICE_CONFIG_RANGE_QUERY, serde_json::json!({}))
        .await?;
    let range = data
        .pointer("/serviceConfig/eventsAvailableRange")
        .or_else(|| data.pointer("/serviceConfig/availableRange"))
        .ok_or_else(|| anyhow!("missing serviceConfig events availableRange"))?;
    let first = range
        .pointer("/first/sequenceNumber")
        .and_then(|v| v.as_u64());
    let last = range
        .pointer("/last/sequenceNumber")
        .and_then(|v| v.as_u64());
    Ok(GraphqlAvailableRange {
        first_checkpoint: first,
        last_checkpoint: last,
    })
}

fn is_invalid_cursor_error(err: &anyhow::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("cursor") && (msg.contains("invalid") || msg.contains("expired"))
}

/// Scan all live pool objects for a DEX (authoritative topology bootstrap).
#[allow(clippy::too_many_arguments)]
pub async fn scan_dex_object_bootstrap<F, Fut, A>(
    spec: &DexDiscoverySpec,
    client: &dyn SuiClientTrait,
    postgres: &dyn PostgresStorageTrait,
    config: &StaticWorkerConfig,
    mut on_pool: F,
    mut after_commit: Option<A>,
) -> Result<DiscoveryScanStats>
where
    F: FnMut(DiscoveryObjectRef) -> Fut,
    Fut: std::future::Future<Output = Result<DiscoveryPageCommit>>,
    A: FnMut(&DiscoveryPageCommit),
{
    let mut stats = DiscoveryScanStats::default();
    let mut progress = postgres
        .get_pool_discovery_progress(spec.dex_name, OBJECT_BOOTSTRAP_SOURCE_KEY)
        .await?
        .unwrap_or_else(|| PoolDiscoveryProgress::new_object_bootstrap(spec.dex_name));

    let mut cursor = progress.page_cursor.clone();

    for page_idx in 0..config.discovery_max_pages_per_scan {
        let variables = discover_objects_variables(
            spec.pool_object_type,
            config.discovery_page_size,
            cursor.as_deref(),
        );

        let data = match client
            .query_graphql_with_variables(DISCOVER_OBJECTS_QUERY, variables)
            .await
        {
            Ok(data) => data,
            Err(err) if is_invalid_cursor_error(&err) => {
                tracing::warn!(
                    dex = spec.dex_name,
                    "object scan invalid cursor; restarting generation {}",
                    progress.generation + 1
                );
                progress.generation += 1;
                progress.page_cursor = None;
                progress.bootstrap_complete = false;
                cursor = None;
                continue;
            }
            Err(err) => return Err(err),
        };

        let page = parse_object_page(&data, ALL_DISCOVERY_SPECS)?;
        stats.pages_scanned += 1;
        metrics::record_discovery_page_scanned(spec.dex_name, DiscoverySource::ObjectBootstrap);

        if page.objects.is_empty() && !page.has_next_page {
            // Empty page does not mark bootstrap complete — contract/object drift signal.
            tracing::warn!(
                dex = spec.dex_name,
                "object bootstrap page empty; bootstrap not marked complete"
            );
            let commit = DiscoveryPageCommit {
                progress: progress.clone(),
                ..Default::default()
            };
            postgres.commit_discovery_page(&commit).await?;
            if let Some(ref mut hook) = after_commit {
                hook(&commit);
            }
            break;
        }

        let mut page_commit = DiscoveryPageCommit {
            progress: progress.clone(),
            ..Default::default()
        };

        let mut seen_pools = HashSet::new();
        for object_ref in &page.objects {
            stats.objects_seen += 1;
            metrics::record_discovery_object_scanned(spec.dex_name);

            if let Some(err) = &object_ref.parse_error {
                page_commit.failures.push(PoolDiscoveryFailure {
                    dex_name: spec.dex_name.to_string(),
                    event_id: format!("object:{}", object_ref.pool_id),
                    pool_id: Some(object_ref.pool_id.clone()),
                    reason: err.clone(),
                    attempts: 1,
                });
                metrics::record_discovery_failure(spec.dex_name, "parse");
                continue;
            }

            if !seen_pools.insert(object_ref.pool_id.clone()) {
                stats.pools_reconciled += 1;
                continue;
            }

            match on_pool(object_ref.clone()).await {
                Ok(commit) => {
                    stats.pools_registered += commit.pools.len() as u32;
                    for _pool in &commit.pools {
                        metrics::record_discovery_pool(
                            spec.dex_name,
                            DiscoverySource::ObjectBootstrap,
                        );
                    }
                    page_commit.pools.extend(commit.pools);
                    page_commit.tokens.extend(commit.tokens);
                    page_commit.failures.extend(commit.failures);
                    page_commit
                        .resolved_failure_ids
                        .extend(commit.resolved_failure_ids);
                    progress.pools_discovered += commit.progress.pools_discovered;
                }
                Err(e) => {
                    page_commit.failures.push(PoolDiscoveryFailure {
                        dex_name: spec.dex_name.to_string(),
                        event_id: format!("object:{}", object_ref.pool_id),
                        pool_id: Some(object_ref.pool_id.clone()),
                        reason: e.to_string(),
                        attempts: 1,
                    });
                    stats.failures_recorded += 1;
                    metrics::record_discovery_failure(spec.dex_name, "hydrate");
                }
            }
        }

        progress.page_cursor = if page.has_next_page {
            page.end_cursor.clone()
        } else {
            None
        };
        if !page.has_next_page {
            progress.bootstrap_complete = true;
            metrics::set_discovery_bootstrap_complete(spec.dex_name, true);
        }

        page_commit.progress = progress.clone();
        postgres.commit_discovery_page(&page_commit).await?;
        if let Some(ref mut hook) = after_commit {
            hook(&page_commit);
        }

        cursor = progress.page_cursor.clone();
        if !page.has_next_page || page_idx + 1 >= config.discovery_max_pages_per_scan {
            break;
        }

        if config.discovery_inter_page_ms > 0 {
            tokio::time::sleep(Duration::from_millis(config.discovery_inter_page_ms)).await;
        }
    }

    Ok(stats)
}

/// Scan one DEX spec across all configured create-pool event types (incremental).
#[allow(clippy::too_many_arguments)]
pub async fn scan_dex_events<F, Fut, A>(
    spec: &DexDiscoverySpec,
    client: &dyn SuiClientTrait,
    postgres: &dyn PostgresStorageTrait,
    config: &StaticWorkerConfig,
    event_range: &GraphqlAvailableRange,
    mut on_event: F,
    mut after_commit: Option<A>,
) -> Result<DiscoveryScanStats>
where
    F: FnMut(DiscoveryEventRef) -> Fut,
    Fut: std::future::Future<Output = Result<DiscoveryPageCommit>>,
    A: FnMut(&DiscoveryPageCommit),
{
    let mut total = DiscoveryScanStats::default();
    for event_type in spec.create_pool_event_types {
        let stats = scan_single_event_type(
            spec,
            event_type,
            client,
            postgres,
            config,
            event_range,
            &mut on_event,
            after_commit.as_mut(),
        )
        .await?;
        total.pages_scanned += stats.pages_scanned;
        total.events_seen += stats.events_seen;
        total.pools_registered += stats.pools_registered;
        total.failures_recorded += stats.failures_recorded;
    }
    Ok(total)
}

#[allow(clippy::too_many_arguments)]
async fn scan_single_event_type<F, Fut, A>(
    spec: &DexDiscoverySpec,
    event_type: &str,
    client: &dyn SuiClientTrait,
    postgres: &dyn PostgresStorageTrait,
    config: &StaticWorkerConfig,
    event_range: &GraphqlAvailableRange,
    on_event: &mut F,
    mut after_commit: Option<&mut A>,
) -> Result<DiscoveryScanStats>
where
    F: FnMut(DiscoveryEventRef) -> Fut,
    Fut: std::future::Future<Output = Result<DiscoveryPageCommit>>,
    A: FnMut(&DiscoveryPageCommit),
{
    let mut stats = DiscoveryScanStats::default();
    let mut progress = postgres
        .get_pool_discovery_progress(spec.dex_name, event_type)
        .await?
        .unwrap_or_else(|| PoolDiscoveryProgress::new_event_incremental(spec.dex_name, event_type));

    let mut cursor = progress.page_cursor.clone();
    let after_checkpoint = if progress.backfill_complete {
        progress.last_checkpoint
    } else {
        None
    };

    for page_idx in 0..config.discovery_max_pages_per_scan {
        let variables = discover_events_variables(
            event_type,
            config.discovery_page_size,
            cursor.as_deref(),
            after_checkpoint,
        );

        let data = match client
            .query_graphql_with_variables(DISCOVER_EVENTS_QUERY, variables)
            .await
        {
            Ok(data) => data,
            Err(err) if is_invalid_cursor_error(&err) => {
                tracing::warn!(
                    dex = spec.dex_name,
                    event_type,
                    "event scan invalid cursor; resuming from last checkpoint {:?}",
                    progress.last_checkpoint
                );
                progress.page_cursor = None;
                cursor = None;
                continue;
            }
            Err(err) => return Err(err),
        };

        let page = parse_event_page(&data, ALL_DISCOVERY_SPECS)?;
        stats.pages_scanned += 1;
        metrics::record_discovery_page_scanned(spec.dex_name, DiscoverySource::EventIncremental);

        let mut page_commit = DiscoveryPageCommit {
            progress: progress.clone(),
            ..Default::default()
        };
        let mut max_checkpoint = progress.last_checkpoint;
        let mut saw_events = false;

        for event_ref in &page.events {
            saw_events = true;
            stats.events_seen += 1;
            metrics::record_discovery_event_scanned(spec.dex_name);

            if let Some(err) = &event_ref.parse_error {
                page_commit.failures.push(PoolDiscoveryFailure {
                    dex_name: spec.dex_name.to_string(),
                    event_id: event_ref.event_id.clone(),
                    pool_id: None,
                    reason: err.clone(),
                    attempts: 1,
                });
                metrics::record_discovery_failure(spec.dex_name, "parse");
                continue;
            }

            if event_ref.pool_ref.is_some() {
                match on_event(event_ref.clone()).await {
                    Ok(commit) => {
                        stats.pools_registered += commit.pools.len() as u32;
                        for _ in &commit.pools {
                            metrics::record_discovery_pool(
                                spec.dex_name,
                                DiscoverySource::EventIncremental,
                            );
                        }
                        page_commit.pools.extend(commit.pools);
                        page_commit.tokens.extend(commit.tokens);
                        page_commit.failures.extend(commit.failures);
                        page_commit
                            .resolved_failure_ids
                            .extend(commit.resolved_failure_ids);
                        progress.pools_discovered += commit.progress.pools_discovered;
                    }
                    Err(e) => {
                        let pool_id = event_ref.pool_ref.as_ref().map(|m| m.pool_id.clone());
                        page_commit.failures.push(PoolDiscoveryFailure {
                            dex_name: spec.dex_name.to_string(),
                            event_id: event_ref.event_id.clone(),
                            pool_id,
                            reason: e.to_string(),
                            attempts: 1,
                        });
                        stats.failures_recorded += 1;
                        metrics::record_discovery_failure(spec.dex_name, "hydrate");
                    }
                }
            }

            if let Some(cp) = event_ref.pool_ref.as_ref().and_then(|m| m.checkpoint) {
                max_checkpoint = Some(max_checkpoint.map_or(cp, |prev| prev.max(cp)));
            }
        }

        progress.last_checkpoint = max_checkpoint.or(progress.last_checkpoint);
        progress.page_cursor = if page.has_next_page {
            page.end_cursor.clone()
        } else {
            None
        };

        if !page.has_next_page {
            progress.backfill_complete = true;
            if !saw_events && event_range.first_checkpoint.is_some() {
                progress.retention_limited = true;
                metrics::set_discovery_retention_limited(spec.dex_name, true);
            }
        }

        if let Some(cp) = progress.last_checkpoint {
            metrics::set_discovery_checkpoint(spec.dex_name, cp as i64);
        }

        page_commit.progress = progress.clone();
        postgres.commit_discovery_page(&page_commit).await?;
        if let Some(hook) = after_commit.as_mut() {
            hook(&page_commit);
        }

        cursor = progress.page_cursor.clone();
        if !page.has_next_page || page_idx + 1 >= config.discovery_max_pages_per_scan {
            break;
        }

        if config.discovery_inter_page_ms > 0 {
            tokio::time::sleep(Duration::from_millis(config.discovery_inter_page_ms)).await;
        }
    }

    Ok(stats)
}

/// Reconcile existing PostgreSQL pools by re-fetching on-chain state (no deletions).
pub async fn reconcile_existing_pools<F, Fut>(
    postgres: &dyn PostgresStorageTrait,
    mut hydrate_pool: F,
) -> Result<u32>
where
    F: FnMut(&crate::models::PoolState) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let pools = postgres.list_pools().await?;
    let mut updated = 0u32;
    for pool in pools {
        if hydrate_pool(&pool).await.is_ok() {
            updated += 1;
        }
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::fixtures;
    use crate::storage::postgres::tests::InMemoryPostgresStorage;
    use crate::sui_client::tests::MockSuiClient;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_scan_dex_events_with_mock_graphql() {
        let pg = Arc::new(InMemoryPostgresStorage::new());
        let sui = Arc::new(MockSuiClient::new());
        let call = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let call_clone = call.clone();
        *sui.query_graphql_with_variables_mock.lock().unwrap() = Box::new(move |query, _| {
            if query.contains("objects") {
                return Ok(fixtures::cetus_object_graphql_response());
            }
            let n = call_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if n == 1 {
                Ok(fixtures::two_page_graphql_response())
            } else {
                Ok(fixtures::second_page_graphql_response())
            }
        });

        let config = StaticWorkerConfig {
            discovery_page_size: 10,
            discovery_max_pages_per_scan: 5,
            discovery_inter_page_ms: 0,
            ..StaticWorkerConfig::default()
        };
        let range = GraphqlAvailableRange {
            first_checkpoint: Some(1),
            last_checkpoint: Some(999),
        };

        let registered = Arc::new(std::sync::Mutex::new(Vec::new()));
        let reg = registered.clone();
        let stats = scan_dex_events(
            &ALL_DISCOVERY_SPECS[0],
            sui.as_ref(),
            pg.as_ref(),
            &config,
            &range,
            move |ev| {
                let reg = reg.clone();
                async move {
                    let mut commit = DiscoveryPageCommit::default();
                    if let Some(meta) = ev.pool_ref {
                        reg.lock().unwrap().push(meta.pool_id.clone());
                        commit.progress.pools_discovered = 1;
                    }
                    Ok(commit)
                }
            },
            None::<fn(&DiscoveryPageCommit)>,
        )
        .await
        .unwrap();

        assert_eq!(stats.pages_scanned, 2);
        assert_eq!(registered.lock().unwrap().len(), 2);
        let progress = pg
            .get_pool_discovery_progress("Cetus", ALL_DISCOVERY_SPECS[0].create_pool_event_types[0])
            .await
            .unwrap()
            .unwrap();
        assert!(progress.backfill_complete);
    }

    #[tokio::test]
    async fn test_object_bootstrap_pagination() {
        let pg = Arc::new(InMemoryPostgresStorage::new());
        let sui = Arc::new(MockSuiClient::new());
        let call = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let call_clone = call.clone();
        *sui.query_graphql_with_variables_mock.lock().unwrap() = Box::new(move |query, _| {
            assert!(query.contains("objects"));
            let n = call_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            if n == 1 {
                Ok(fixtures::two_page_object_graphql_response())
            } else {
                Ok(fixtures::second_object_page_graphql_response())
            }
        });

        let config = StaticWorkerConfig {
            discovery_page_size: 10,
            discovery_max_pages_per_scan: 5,
            discovery_inter_page_ms: 0,
            ..StaticWorkerConfig::default()
        };

        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen_clone = seen.clone();
        let stats = scan_dex_object_bootstrap(
            &ALL_DISCOVERY_SPECS[0],
            sui.as_ref(),
            pg.as_ref(),
            &config,
            move |obj| {
                let seen = seen_clone.clone();
                async move {
                    seen.lock().unwrap().push(obj.pool_id);
                    Ok(DiscoveryPageCommit::default())
                }
            },
            None::<fn(&DiscoveryPageCommit)>,
        )
        .await
        .unwrap();

        assert_eq!(stats.pages_scanned, 2);
        assert_eq!(seen.lock().unwrap().len(), 2);
        let progress = pg
            .get_pool_discovery_progress("Cetus", OBJECT_BOOTSTRAP_SOURCE_KEY)
            .await
            .unwrap()
            .unwrap();
        assert!(progress.bootstrap_complete);
    }

    #[tokio::test]
    async fn test_empty_event_page_does_not_false_complete_retention() {
        let pg = Arc::new(InMemoryPostgresStorage::new());
        let sui = Arc::new(MockSuiClient::new());
        *sui.query_graphql_with_variables_mock.lock().unwrap() = Box::new(|_, _| {
            Ok(serde_json::json!({
                "events": {
                    "pageInfo": { "hasNextPage": false, "endCursor": null },
                    "nodes": []
                }
            }))
        });

        let config = StaticWorkerConfig::default();
        let range = GraphqlAvailableRange {
            first_checkpoint: Some(100),
            last_checkpoint: Some(200),
        };

        scan_dex_events(
            &ALL_DISCOVERY_SPECS[2],
            sui.as_ref(),
            pg.as_ref(),
            &config,
            &range,
            |_| async { Ok(DiscoveryPageCommit::default()) },
            None::<fn(&DiscoveryPageCommit)>,
        )
        .await
        .unwrap();

        let progress = pg
            .get_pool_discovery_progress("Magma", ALL_DISCOVERY_SPECS[2].create_pool_event_types[0])
            .await
            .unwrap()
            .unwrap();
        assert!(progress.backfill_complete);
        assert!(progress.retention_limited);
    }
}
