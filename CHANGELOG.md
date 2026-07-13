# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] — 2026-07-13

### Added

- Initial open-source release of **DevilRay** (`devilray-sui`).
- Multi-DEX CLMM collectors for Cetus, Turbos, Magma Finance, and Momentum.
- Tick-aware smart order routing, slippage model, and convex order split.
- GraphQL object-bootstrap pool discovery with event-incremental acceleration.
- Worker pipeline with Redis hot path, PostgreSQL cold path, and optional ClickHouse analytics.
- HTTP API: `/health`, `/readyz`, `/api/v1/tokens`, `/metrics`, `/api/quote`, `/api/build_tx`.
- WebSocket live `pool_update` subscriptions.
- Canonical unsigned transaction BCS prototype with local sign/verify unit tests.
- Docker Compose stack, DLQ replay tooling, and GitHub Actions CI.
- English documentation set under `docs/` (architecture, API, development, operations, roadmap).

### Known limitations

- On-chain execution, dry-run, automatic coin selection, and stale-quote guards are not verified in this release.