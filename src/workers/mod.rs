pub mod db_writer;
pub mod dynamic_worker;
pub mod static_worker;

#[derive(Debug, Clone)]
pub struct DynamicWorkerConfig {
    /// Number of concurrent tokio tasks to process updates
    pub num_workers: usize,
    /// Time to wait between polling active pools (in milliseconds)
    pub poll_interval_ms: u64,
    /// Maximum retries for RPC network errors
    pub max_retries: usize,
    /// Initial backoff time for retries (in milliseconds)
    pub retry_backoff_ms: u64,
    /// WebSocket URL to subscribe to live events (Push)
    pub websocket_url: String,
    /// Delay before attempting WebSocket reconnection (in milliseconds)
    pub ws_reconnect_interval_ms: u64,
}

impl Default for DynamicWorkerConfig {
    fn default() -> Self {
        Self {
            num_workers: 4,
            poll_interval_ms: 2000, // Pull polling fallback interval (2 seconds)
            max_retries: 3,
            retry_backoff_ms: 200,
            websocket_url: "wss://fullnode.mainnet.sui.io:443".to_string(),
            ws_reconnect_interval_ms: 3000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StaticWorkerConfig {
    /// Number of concurrent tasks to scan for new pools
    pub num_workers: usize,
    /// Scanning interval (in seconds)
    pub scan_interval_secs: u64,
    /// GraphQL page size per discovery request
    pub discovery_page_size: u32,
    /// Max pages fetched per DEX per scan cycle (bounded pagination)
    pub discovery_max_pages_per_scan: u32,
    /// Delay between GraphQL pages (milliseconds)
    pub discovery_inter_page_ms: u64,
    /// Max concurrent on-chain pool hydrations per discovery page
    pub discovery_hydrate_concurrency: usize,
    /// Run reconciliation (re-fetch existing pools) every N scan cycles
    pub reconciliation_every_n_scans: u32,
}

impl Default for StaticWorkerConfig {
    fn default() -> Self {
        Self {
            num_workers: 1,
            scan_interval_secs: 3600, // hourly
            discovery_page_size: 50,
            discovery_max_pages_per_scan: 100,
            discovery_inter_page_ms: 100,
            reconciliation_every_n_scans: 1,
            discovery_hydrate_concurrency: 8,
        }
    }
}
