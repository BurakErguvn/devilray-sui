use crate::discovery::progress::DiscoverySource;
use axum::http::header::CONTENT_TYPE;
use prometheus::{
    Encoder, Histogram, IntCounter, IntCounterVec, IntGauge, IntGaugeVec, TextEncoder,
    default_registry, register_histogram, register_int_counter, register_int_counter_vec,
    register_int_gauge, register_int_gauge_vec,
};
use std::sync::LazyLock;
use std::time::Instant;

static QUOTE_REQUESTS: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "quote_requests_total",
        "Total /api/quote requests",
        &["status"]
    )
    .expect("register quote_requests_total")
});

static QUOTE_LATENCY: LazyLock<Histogram> = LazyLock::new(|| {
    register_histogram!(
        "quote_latency_seconds",
        "/api/quote latency",
        vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5]
    )
    .expect("register quote_latency_seconds")
});

static ACTIVE_POOLS: LazyLock<IntGauge> = LazyLock::new(|| {
    register_int_gauge!("active_pools_count", "Active (non-paused) pool count")
        .expect("register active_pools_count")
});

static DLQ_PUSHED: LazyLock<IntCounter> = LazyLock::new(|| {
    register_int_counter!("dlq_pushed_total", "Messages pushed to dead-letter queue")
        .expect("register dlq_pushed_total")
});

static DISCOVERY_POOLS: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "discovery_pools_discovered_total",
        "Pools registered via GraphQL discovery",
        &["dex", "source"]
    )
    .expect("register discovery_pools_discovered_total")
});

static DISCOVERY_EVENTS: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "discovery_events_scanned_total",
        "Pool-create events scanned",
        &["dex"]
    )
    .expect("register discovery_events_scanned_total")
});

static DISCOVERY_OBJECTS: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "discovery_objects_scanned_total",
        "Pool objects scanned via GraphQL object pagination",
        &["dex"]
    )
    .expect("register discovery_objects_scanned_total")
});

static DISCOVERY_PAGES: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "discovery_pages_scanned_total",
        "GraphQL discovery pages fetched",
        &["dex", "source"]
    )
    .expect("register discovery_pages_scanned_total")
});

static DISCOVERY_FAILURES: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "discovery_scan_failures_total",
        "Discovery parse/fetch failures",
        &["dex", "kind"]
    )
    .expect("register discovery_scan_failures_total")
});

static DISCOVERY_CHECKPOINT: LazyLock<IntGaugeVec> = LazyLock::new(|| {
    register_int_gauge_vec!(
        "discovery_checkpoint_sequence",
        "Last committed discovery checkpoint per DEX",
        &["dex"]
    )
    .expect("register discovery_checkpoint_sequence")
});

static DISCOVERY_BOOTSTRAP_COMPLETE: LazyLock<IntGaugeVec> = LazyLock::new(|| {
    register_int_gauge_vec!(
        "discovery_bootstrap_complete",
        "Object bootstrap completion per DEX (1=complete)",
        &["dex"]
    )
    .expect("register discovery_bootstrap_complete")
});

static DISCOVERY_RETENTION_LIMITED: LazyLock<IntGaugeVec> = LazyLock::new(|| {
    register_int_gauge_vec!(
        "discovery_event_retention_limited",
        "Event backfill retention-limited per DEX (1=limited)",
        &["dex"]
    )
    .expect("register discovery_event_retention_limited")
});

static DISCOVERY_UNRESOLVED_FAILURES: LazyLock<IntGaugeVec> = LazyLock::new(|| {
    register_int_gauge_vec!(
        "discovery_unresolved_failures",
        "Unresolved discovery failures per DEX",
        &["dex"]
    )
    .expect("register discovery_unresolved_failures")
});

fn source_label(source: DiscoverySource) -> &'static str {
    match source {
        DiscoverySource::ObjectBootstrap => "object",
        DiscoverySource::EventIncremental => "event",
    }
}

pub fn record_quote_ok(latency: std::time::Duration, active_pool_count: i64) {
    QUOTE_REQUESTS.with_label_values(&["ok"]).inc();
    QUOTE_LATENCY.observe(latency.as_secs_f64());
    ACTIVE_POOLS.set(active_pool_count);
}

pub fn record_quote_error(latency: std::time::Duration) {
    QUOTE_REQUESTS.with_label_values(&["error"]).inc();
    QUOTE_LATENCY.observe(latency.as_secs_f64());
}

pub fn quote_timer() -> Instant {
    Instant::now()
}

pub fn record_dlq_push() {
    DLQ_PUSHED.inc();
}

pub fn record_discovery_pool(dex: &str, source: DiscoverySource) {
    DISCOVERY_POOLS
        .with_label_values(&[dex, source_label(source)])
        .inc();
}

pub fn record_discovery_event_scanned(dex: &str) {
    DISCOVERY_EVENTS.with_label_values(&[dex]).inc();
}

pub fn record_discovery_object_scanned(dex: &str) {
    DISCOVERY_OBJECTS.with_label_values(&[dex]).inc();
}

pub fn record_discovery_page_scanned(dex: &str, source: DiscoverySource) {
    DISCOVERY_PAGES
        .with_label_values(&[dex, source_label(source)])
        .inc();
}

pub fn record_discovery_failure(dex: &str, kind: &str) {
    DISCOVERY_FAILURES.with_label_values(&[dex, kind]).inc();
}

pub fn set_discovery_checkpoint(dex: &str, checkpoint: i64) {
    DISCOVERY_CHECKPOINT
        .with_label_values(&[dex])
        .set(checkpoint);
}

pub fn set_discovery_bootstrap_complete(dex: &str, complete: bool) {
    DISCOVERY_BOOTSTRAP_COMPLETE
        .with_label_values(&[dex])
        .set(i64::from(complete));
}

pub fn set_discovery_retention_limited(dex: &str, limited: bool) {
    DISCOVERY_RETENTION_LIMITED
        .with_label_values(&[dex])
        .set(i64::from(limited));
}

pub fn set_discovery_unresolved_failures(dex: &str, count: i64) {
    DISCOVERY_UNRESOLVED_FAILURES
        .with_label_values(&[dex])
        .set(count);
}

pub async fn handle_metrics() -> impl axum::response::IntoResponse {
    let body = metrics_body();
    ([(CONTENT_TYPE, "text/plain; version=0.0.4")], body)
}

fn ensure_metrics_registered() {
    QUOTE_REQUESTS.with_label_values(&["ok"]);
    QUOTE_REQUESTS.with_label_values(&["error"]);
    QUOTE_LATENCY.observe(0.0);
    ACTIVE_POOLS.set(ACTIVE_POOLS.get());
    let _ = &*DLQ_PUSHED;
    DISCOVERY_POOLS.with_label_values(&["Cetus", "object"]);
    DISCOVERY_EVENTS.with_label_values(&["Cetus"]);
    DISCOVERY_OBJECTS.with_label_values(&["Cetus"]);
    DISCOVERY_PAGES.with_label_values(&["Cetus", "object"]);
    DISCOVERY_FAILURES.with_label_values(&["Cetus", "parse"]);
    DISCOVERY_CHECKPOINT.with_label_values(&["Cetus"]).set(0);
    DISCOVERY_BOOTSTRAP_COMPLETE
        .with_label_values(&["Cetus"])
        .set(0);
    DISCOVERY_RETENTION_LIMITED
        .with_label_values(&["Cetus"])
        .set(0);
    DISCOVERY_UNRESOLVED_FAILURES
        .with_label_values(&["Cetus"])
        .set(0);
}

fn metrics_body() -> String {
    ensure_metrics_registered();
    let encoder = TextEncoder::new();
    let metric_families = default_registry().gather();
    let mut buf = Vec::new();
    encoder.encode(&metric_families, &mut buf).unwrap();
    String::from_utf8(buf).unwrap()
}

#[cfg(test)]
pub(crate) fn metrics_body_for_test() -> String {
    metrics_body()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::websocket::ServerAppState;
    use crate::storage::postgres::tests::InMemoryPostgresStorage;
    use crate::storage::redis::tests::InMemoryRedisCache;
    use axum::{Router, routing::get};
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::broadcast;

    #[tokio::test]
    async fn test_metrics_endpoint() {
        let (broadcast_tx, _) = broadcast::channel(10);
        let pg_db = Arc::new(InMemoryPostgresStorage::new());
        let redis_cache = Arc::new(InMemoryRedisCache::new());
        let app_state = ServerAppState::new(
            broadcast_tx,
            pg_db,
            redis_cache,
            Arc::new(std::sync::atomic::AtomicBool::new(true)),
            Arc::new(crate::sui_client::tests::MockSuiClient::new()),
        );
        let app = Router::new()
            .route("/metrics", get(handle_metrics))
            .with_state(app_state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::new();
        let res = client
            .get(format!("http://{}/metrics", addr))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let body = res.text().await.unwrap();
        assert!(body.contains("quote_requests_total"));
        assert!(body.contains("discovery_objects_scanned_total"));
    }

    #[tokio::test]
    async fn test_quote_metrics_family_present_after_record() {
        record_quote_ok(std::time::Duration::from_millis(1), 3);
        let body = metrics_body();
        assert!(body.contains("quote_requests_total"));
        assert!(body.contains("active_pools_count"));
    }
}
