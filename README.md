# DevilRay

[![CI](https://github.com/BurakErguvn/devilray-sui/actions/workflows/ci.yml/badge.svg)](https://github.com/BurakErguvn/devilray-sui/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE)

**Experimental** open-source backend for aggregating Sui CLMM liquidity across multiple DEXes.

DevilRay discovers pools, caches live state, routes swaps with tick-aware simulation, and assembles **unsigned** canonical Sui transaction bytes for client-side signing.

> **Status:** early development. Quotes and PTB bytes are produced from cached or simulated pool state. On-chain execution, dry-run validation, and production-grade reliability are **not** verified yet. Do not use with real funds without independent review.

## What works today

- **Multi-DEX CLMM collectors** — Cetus, Turbos, Magma Finance, Momentum
- **Tick-aware routing** — V3 tick-crossing simulation with within-tick fallback
- **Slippage model** — basis-point tolerance, compound per-hop `min_out`, dynamic `sqrt_price_limit`
- **Smart order routing** — Dijkstra (small amounts), hop-limited DFS (large amounts), convex order split with gas-aware pruning
- **Pool discovery** — GraphQL object-bootstrap topology + event-incremental scan; `/readyz` topology gate
- **Worker pipeline** — static discovery, dynamic WS/poll updates, Redis queue, PostgreSQL / optional ClickHouse
- **HTTP + WebSocket API** — quote, build_tx, tokens, health, readiness, Prometheus metrics, live `pool_update`
- **Canonical transaction prototype** — BCS round-trip + local Ed25519 sign/verify tests via deterministic `InMemorySuiClient`
- **Ops tooling** — Docker Compose, DLQ + `replay_dlq`, CI (fmt / clippy / unit + PG+Redis integration)

## What is not verified yet

- Mainnet or `sui-fork` **build → sign → execute** (#57)
- API dry-run / `devInspect` before signing (#26)
- Automatic coin-object selection (#27)
- Stale-quote / state-age guards (#33)
- DeepBook / FlowX and broader liquidity coverage (#30)
- Auth, rate limits, CORS for browser clients (#13, #56)
- TypeScript SDK / wallet adapters (#28)

See the full [roadmap](docs/ROADMAP.md).

## Quick start

### Docker (recommended)

```bash
cp .env.example .env
docker compose up --build
```

API listens on `http://localhost:3000`.

### Local development

```bash
docker compose -f docker-compose.test.yml up -d --wait test-postgres test-redis

export DATABASE_URL=postgres://test_user:test_password@localhost:5433/test_db
export REDIS_URL=redis://localhost:6379
export RUST_LOG=devilray_sui=info

cargo run
```

Press `Ctrl+C` for graceful shutdown.

## Architecture

```
Sui RPC / GraphQL / WS
        │
   Collectors (4 DEX)
        │
 Static + Dynamic workers ──► Redis (hot path + queue)
        │                           │
        │                     DatabaseWriter
        │                      │         │
        │                 PostgreSQL   ClickHouse
        ▼
   Axum API  (/quote, /build_tx, /ws, /metrics)
```

Details and Mermaid diagrams: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## API at a glance

| Method | Path | Description |
| --- | --- | --- |
| `GET` | `/health` | Process liveness |
| `GET` | `/readyz` | Topology bootstrap + non-empty pool catalog |
| `GET` | `/api/v1/tokens` | Registered tokens |
| `GET` | `/metrics` | Prometheus metrics |
| `GET` | `/api/quote` | Best aggregate quote |
| `POST` | `/api/build_tx` | Build **unsigned** canonical transaction bytes |
| `GET` | `/ws` | Live `pool_update` subscriptions |

Full contract and examples: [docs/API.md](docs/API.md).

### Quote example

```bash
curl -sG "http://localhost:3000/api/quote" \
  --data-urlencode "from_token=0x2::sui::SUI" \
  --data-urlencode "to_token=0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN" \
  --data-urlencode "amount=1000000000"
```

### Build transaction (unsigned)

`POST /api/build_tx` returns `transaction_data_bcs`, `transaction_digest`, gas metadata, and a debug symbolic PTB. DevilRay does **not** call `executeTransactionBlock`.

## Environment

Copy [`.env.example`](.env.example). Important variables:

| Variable | Required | Default | Description |
| --- | --- | --- | --- |
| `DATABASE_URL` | yes | — | PostgreSQL connection string |
| `REDIS_URL` | yes | — | Redis connection string |
| `CLICKHOUSE_URL` | no | — | ClickHouse HTTP URL; omit for mock analytics |
| `RPC_URL` | no | Sui mainnet fullnode | JSON-RPC endpoint |
| `GRAPHQL_URL` | no | Sui mainnet GraphQL | GraphQL endpoint |
| `WEBSOCKET_URL` | no | Sui mainnet WS | WebSocket endpoint |
| `BIND_ADDR` | no | `0.0.0.0:3000` | HTTP/WS bind address |
| `RUST_LOG` | no | `devilray_sui=info` | Tracing filter |

Defaults point at **public Sui mainnet** endpoints for read-side discovery. Respect provider rate limits and terms of use.

## Development

```bash
make check    # cargo check
make test     # unit tests (InMemory mocks)
make lint     # fmt --check + clippy -D warnings
make ci       # lint + unit tests
make test-integration
```

More: [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) · [docs/OPERATIONS.md](docs/OPERATIONS.md)

## Documentation

| Doc | Description |
| --- | --- |
| [docs/README.md](docs/README.md) | Documentation index |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Components, data flow, diagrams |
| [docs/API.md](docs/API.md) | REST / WebSocket contract |
| [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) | Local setup, tests, conventions |
| [docs/OPERATIONS.md](docs/OPERATIONS.md) | Docker, metrics, DLQ |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Backlog and release gates |
| [docs/ROUTING_PERFORMANCE.md](docs/ROUTING_PERFORMANCE.md) | Pathfinding micro-benchmarks |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Please open issues for bugs and feature ideas; security reports go through [SECURITY.md](SECURITY.md).

## License

Licensed under the [Apache License, Version 2.0](LICENSE).

## Disclaimer

DevilRay is experimental software. Quotes may be optimistic when tick data is incomplete or pool state is stale. Generated transaction bytes are not guaranteed to execute on any network.