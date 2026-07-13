# Contributing to DevilRay

Thanks for your interest in contributing.

## Prerequisites

- Rust **1.85+** (CI uses the stable toolchain; Docker image may pin a newer patch)
- Docker + Docker Compose (integration tests and full stack)
- `make` (optional but convenient)

## Development workflow

1. Fork and clone the repository.
2. Copy `.env.example` to `.env` if you run the full Docker stack.
3. For unit tests only:

```bash
make ci
```

4. For storage/queue changes, also run integration tests:

```bash
make test-integration
```

5. Open a pull request against `main`.

## Project invariants

Please do not break these without discussion:

1. **Hot path = Redis** — pool state TTL ~300s, list queue (`rpush`/`lpop`), gas price cache.
2. **Cold path = PostgreSQL** — tokens, pools, ticks, discovery progress, system config.
3. **Analytics = ClickHouse** — optional `swap_events`.
4. **Dynamic worker order** — `fetch_pool` → Redis `set` → queue `publish` → `broadcast`.
5. **Graceful shutdown** — every long-lived loop must respect `tokio::sync::watch`.
6. **DI via traits** — `Arc<dyn Trait>` with a real impl and an `InMemory*` mock for unit tests.
7. **Amounts** — persist and expose `u128` amounts as strings; avoid `f64` amount math.

## Adding a DEX

1. Implement `DexDataCollector` (`fetch_pool`, optionally `fetch_tick_data`).
2. Register discovery specs / swap contracts in `src/discovery/registry.rs`.
3. Extend `dex_swap::build_hop_commands` for the MoveCall pattern.
4. Add fixtures and unit tests; use `probe_*` binaries for live introspection.
5. Update `docs/ROADMAP.md` / supported-DEX docs if the surface changes.

A DEX is “supported” only when collector + quote simulation + PTB layout + tests exist — not when a collector alone is added.

## Testing policy

- New logic modules should include a colocated `#[cfg(test)] mod tests`.
- Unit tests must not hit real infra — use `InMemoryPostgresStorage`, `InMemoryRedisCache`, `InMemoryMessageQueue`, `InMemorySuiClient` / `MockSuiClient`.
- Integration tests are gated behind `--features integration` and skip silently when env vars are unset.

## Documentation

- Keep public docs in English under `docs/`.
- Do not add private notes vaults or non-public personal path references to the public tree.
- Update relevant docs when changing API, architecture, or roadmap status.

## Pull requests

- Prefer focused PRs.
- Ensure `make ci` is green.
- Describe *what* and *why*; link issues when applicable.
- Call out any intentional scope limits (especially around transaction execution).