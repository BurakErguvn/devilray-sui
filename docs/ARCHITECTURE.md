# Architecture

DevilRay runs as a single long-lived daemon: Axum HTTP/WebSocket API, background workers, and Sui clients share one process. Storage is split into three paths.

| Path | Technology | Role |
| --- | --- | --- |
| **Hot** | Redis | Live `PoolState`, topology snapshots, tick cache, list queue, gas price |
| **Cold** | PostgreSQL | Tokens, pools, tick tables, discovery progress, system config |
| **Analytics** | ClickHouse (optional) | `swap_events` MergeTree |

## Component overview

```mermaid
flowchart TB
  subgraph External["External"]
    SuiRPC["Sui JSON-RPC / GraphQL / WS"]
    Client["API clients / wallets"]
  end

  subgraph Daemon["Daemon"]
    subgraph API["API"]
      Quote["quote / build_tx"]
      Info["health / readyz / tokens / metrics"]
      WsApi["websocket /ws"]
    end
    subgraph SOR["Smart order routing"]
      Router["router"]
      Slippage["slippage"]
      DexSwap["dex_swap"]
      TxBuilder["transaction_builder"]
    end
    subgraph Workers["Workers"]
      StaticWM["StaticPoolManager"]
      DynamicWM["DynamicPoolManager"]
      DbWriter["DatabaseWriter"]
    end
    Collectors["DexDataCollector x4"]
    Discovery["object bootstrap + event scan"]
    SuiClient["sui_client"]
  end

  subgraph Storage["Storage"]
    Redis["Redis hot + queue"]
    PG["PostgreSQL cold"]
    CH["ClickHouse analytics"]
  end

  Client --> Quote
  Client --> WsApi
  Client --> Info
  Quote --> Router
  Quote --> DexSwap
  Quote --> TxBuilder
  Quote --> Redis
  Quote --> PG
  Quote --> SuiClient
  StaticWM --> Discovery
  StaticWM --> Collectors
  DynamicWM --> Collectors
  Collectors --> SuiClient
  Discovery --> SuiClient
  StaticWM --> PG
  StaticWM --> Redis
  DynamicWM --> Redis
  DynamicWM -.broadcast.-> WsApi
  DbWriter --> Redis
  DbWriter --> PG
  DbWriter --> CH
  SuiClient --> SuiRPC
```

## Daemon lifecycle

```mermaid
sequenceDiagram
  participant Main as main
  participant Daemon as run_daemon
  participant PG as PostgresDb
  participant RD as RedisCache
  participant CH as ClickhouseClient
  participant SW as StaticPoolManager
  participant DW as DynamicPoolManager
  participant DBW as DatabaseWriter
  participant Axum as AxumServer

  Main->>Daemon: run_daemon(cfg, shutdown_rx)
  Daemon->>PG: connect_with_retry
  Daemon->>RD: connect_with_retry
  Daemon->>CH: connect_or_mock
  Daemon->>SW: spawn
  Daemon->>DW: spawn
  Daemon->>DBW: spawn
  Daemon->>Axum: serve with_graceful_shutdown
  Note over Main: Ctrl+C
  Main->>Daemon: shutdown_tx.send(true)
  Daemon->>SW: await join
  Daemon->>DW: await join
  Daemon->>DBW: await join
  Daemon->>Axum: await join
```

## Dynamic worker invariant

Per pool update task, order is fixed:

```mermaid
sequenceDiagram
  participant W as DynamicWorker
  participant C as DexDataCollector
  participant S as SuiClientTrait
  participant R as RedisCacheTrait
  participant Q as MessageQueueTrait
  participant B as broadcast::Sender

  W->>C: fetch_pool
  C->>S: get_object / parse
  C-->>W: PoolState
  W->>R: set_pool_state
  W->>Q: publish PoolStateUpdate
  W->>B: send(state)
```

Do not reorder these steps.

## Discovery

`StaticPoolManager` treats **object bootstrap** (`objects(filter: { type })`) as authoritative topology. Event scanning is incremental acceleration only; retention gaps report `retention_limited` without wiping topology.

Pages commit atomically to PostgreSQL (pools + tokens + progress + failures), then refresh Redis topology caches and enqueue persistence messages.

## Smart order routing

- Build `TokenGraph` from active pools (paused pools excluded).
- Small amounts → Dijkstra (fee + hop cost).
- Large amounts → hop-limited DFS, then `optimize_order_split` with gas-aware pruning.
- Simulation prefers tick-aware CLMM math when `PoolTickData` exists; otherwise within-tick fallback.
- Slippage uses basis points and `u128` amount math.

## Transaction building

Two layers:

1. **Symbolic PTB** — `dex_swap` emits `PtbCommand` chains (flash-swap patterns for Magma/Momentum; router-style for Turbos; Cetus flash pattern).
2. **Canonical builder** — `transaction_builder` resolves object/gas metadata into `sui_sdk_types::Transaction` BCS + digest.

```mermaid
flowchart TB
  subgraph Offline["Implemented acceptance"]
    A1["Symbolic PTB"]
    A2["Object / gas metadata"]
    A3["Canonical BCS round-trip"]
    A4["Deterministic digest"]
    A5["Local Ed25519 sign/verify tests"]
    A1 --> A2 --> A3 --> A4 --> A5
  end

  subgraph NotYet["Not verified"]
    B1["mainnet / fork execute"]
    B2["package upgrade live matching"]
    B3["devInspect dry-run"]
    B4["automatic coin selection"]
    B5["production acceptance"]
  end

  Offline -.->|"out of scope today"| NotYet
```

`InMemorySuiClient` deliberately refuses `execute_transaction_block`.

## Queue messages

| `QueueMessage` | Persistence target |
| --- | --- |
| `PoolStateUpdate` | PostgreSQL pools |
| `PoolTickDataUpdate` | PostgreSQL tick tables |
| `SwapEventLog` | ClickHouse `swap_events` |

Terminal write failures go to a Redis DLQ after retries; operators can replay with `replay_dlq`.

## Key source files

| Area | Paths |
| --- | --- |
| Entry | `src/main.rs`, `src/daemon.rs` |
| API | `src/api/*` |
| Routing | `src/router.rs`, `src/slippage.rs` |
| PTB | `src/dex_swap.rs`, `src/transaction_builder.rs`, `src/models.rs` |
| Workers | `src/workers/*` |
| Collectors / discovery | `src/collectors/*`, `src/discovery/*` |
| Storage / queue | `src/storage/*`, `src/queue.rs` |
| Sui client | `src/sui_client.rs` |
