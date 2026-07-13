# Development

## Prerequisites

- Rust stable (1.85+; Docker builder may use a newer patch such as 1.96)
- Docker Compose for integration tests / full stack
- Optional ClickHouse for analytics persistence

## Setup

```bash
git clone https://github.com/BurakErguvn/devilray-sui.git
cd devilray-sui
cp .env.example .env   # only needed for docker compose app stack
```

## Common commands

| Command | Purpose |
| --- | --- |
| `make check` | `cargo check` |
| `make test` | Unit tests |
| `make lint` | `fmt --check` + `clippy -D warnings` |
| `make ci` | lint + unit tests |
| `make test-integration` | PG+Redis integration suite |
| `make run` | Start daemon |
| `make docker-up` / `make docker-down` | Full compose stack |

## Test tiers

1. **Unit** — always online; `InMemory*` mocks; no Docker required.
2. **Integration** — `cargo test --features integration` with `DATABASE_URL` + `REDIS_URL`.
3. **Chain execution** — not in default CI; tracked as roadmap #57.

If `DATABASE_URL` / `REDIS_URL` are unset, gated integration paths skip silently.

## Architecture invariants for contributors

1. Dynamic worker order: `fetch_pool` → Redis → queue → broadcast.
2. Persist `u128` amounts as strings.
3. Every storage/queue/client trait needs a real impl + `InMemory` mock.
4. Respect `shutdown_rx` in every loop.
5. Do not claim production execution readiness without #57 + dry-run coverage.

## CLI tools

| Binary | Usage |
| --- | --- |
| `fetch_pool` | `cargo run --bin fetch_pool -- <dex> <pool_id>` |
| `fetch_ticks` | `cargo run --bin fetch_ticks -- <dex> <pool_id>` |
| `probe_ticks` | Live tick probing |
| `probe_swap` | Config / package probing |
| `probe_pool_discovery` | `range \| dex <Name> \| all \| verify [--strict]` |
| `verify_storage` | Storage connectivity checks |
| `benchmark_routing` | Synthetic SOR micro-benchmarks |
| `replay_dlq` | Replay DLQ messages to the main queue |

## Adding a DEX checklist

See [CONTRIBUTING.md](../CONTRIBUTING.md). Minimum bar: collector + tick path (if applicable) + simulation + PTB hop builder + tests.

## Code style

- Prefer `anyhow` / `thiserror` and `tracing` as in existing modules.
- Keep public docs in English.
- Colocate unit tests next to the module under test.
