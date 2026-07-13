# Roadmap

This roadmap is the public backlog for DevilRay. Item IDs (`#N`) are stable.

**Status legend**

| Status | Meaning |
| --- | --- |
| Done | Implemented and covered by tests / CI where applicable |
| Planned | Scoped, not started (or incomplete) |
| Research / deferred | Explicitly out of core path for now |

## North-star metrics (design goals)

These are product goals, not current measured SLOs:

- Net price superiority after gas and fees versus peers
- Quote / build latency (`p50` / `p95` / `p99`) and cache hit rate
- Transaction success rate (dry-run and on-chain), once those paths exist
- State freshness (oldest pool state age at quote time)
- Coverage (DEXes, active pools, pairs, reachable liquidity)
- Conversion funnel: quote → sign → submit → success
- Trust: realized vs quoted output, transparent fees, token risk signals

## A. Completed foundation

| ID | Item | Status |
| --- | --- | --- |
| #1 | Tick-aware CLMM simulation + persistence + fallback | Done |
| #1b | Turbos real tick fetching | Done |
| #1c | Magma & Momentum tick support | Done |
| #2 | Magma & Momentum flash-swap PTB integration | Done |
| #3 | Realistic slippage (`SlippageBps`, compound min_out, dynamic sqrt limit) | Done |
| #5 | CLMM fee-growth metadata | Done |
| #6 | Mainnet object-bootstrap discovery + event incremental | Done |
| #11 | Redis topology / token API cache | Done |
| #12 | Prometheus quote / discovery metrics | Done |
| #16 | Docker multi-stage + Compose | Done |
| #17 | CI (fmt, clippy, unit, integration) | Done |
| #18 | Long-lived daemon entrypoint | Done |
| #20 | Dead-letter queue + replay | Done |
| #22 | Integration test foundation | Done |
| #35 | Canonical Sui TransactionData / BCS **prototype** (offline sign/verify) | Done |

**#35 scope note:** symbolic plan → official transaction bytes, deterministic metadata via `InMemorySuiClient`, BCS round-trip, digest, local Ed25519 tests. **Does not** include live execute or production package-version guarantees.

## B. P0 — Swap correctness and price accuracy

| ID | Item | Status |
| --- | --- | --- |
| #25 | On-chain CLMM parity tests vs SDK / dev-inspect | Planned |
| #26 | Build-tx dry-run + one-shot fallback requote | Planned |
| #27 | Automatic coin-object selection / merge-split | Planned |
| #30 | Broader liquidity (DeepBook, FlowX, …) | Planned |
| #32 | Amount-aware dynamic tick coverage | Planned |
| #33 | State freshness / stale-quote guard | Planned |
| #34 | Net-output optimization (gas + fees) | Planned |
| #36 | Quote / build consistency (quote ID, versions, expiry) | Planned |
| #53 | Hot-pool tick freshness scheduler | Planned |
| #54 | Output-aware candidate routes for small sizes | Planned |
| #57 | Real chain build → sign → execute verification | Planned |

## C. P1 — Speed and reliability

| ID | Item | Status |
| --- | --- | --- |
| #7 | DEX-specific WebSocket event filters + gap fill | Planned |
| #8 | Adaptive polling by activity | Planned |
| #19 | Richer readiness (Redis/PG/RPC/queue lag) | Planned |
| #21 | End-to-end tracing / OpenTelemetry | Planned |
| #37 | Multi-provider RPC strategy | Planned |
| #38 | Incremental graph + hot-route cache | Planned |
| #39 | Capacity / load tests | Planned |
| #40 | Production SLO + alert set | Planned |
| #55 | Gas-price hot path + realistic gas model | Planned |

## D. P1 — Product and adoption

| ID | Item | Status |
| --- | --- | --- |
| #9 | Product endpoints (`/pools`, `/price`, `/routes`) | Planned |
| #10 | Explainable quote response | Planned |
| #28 | TypeScript SDK + wallet adapters | Planned |
| #41 | Transaction lifecycle API | Planned |
| #42 | Token safety controls | Planned |
| #43 | Sponsored tx / gas abstraction evaluation | Planned |
| #44 | Reference frontend | Planned |
| #56 | Browser API contract / CORS | Planned |

## E. P1 — Security, monetization, control

| ID | Item | Status |
| --- | --- | --- |
| #13 | Layered abuse protection | Planned |
| #15 | Typed config / secret management | Planned |
| #45 | Transparent fee + partner attribution | Planned |
| #46 | DEX circuit breaker / feature flags | Planned |
| #47 | Security review scenarios | Planned |

## F. Testing and math assurance

| ID | Item | Status |
| --- | --- | --- |
| #23 | Split optimizer property tests | Planned |
| #24 | Parser fuzzing | Planned |
| #48 | PTB golden / snapshot matrix | Planned |
| #49 | Mainnet shadow quoting worker | Planned |

## G. P2 — Analytics and growth

| ID | Item | Status |
| --- | --- | --- |
| #14 | gRPC / GraphQL API (only if REST is proven insufficient) | Planned |
| #50 | Competitor benchmark system | Planned |
| #51 | Product funnel analytics | Planned |
| #52 | Limit / intent style order options | Planned |

## H. Research / deferred (out of core path)

| ID | Item | Status |
| --- | --- | --- |
| #4 | Arbitrage scanner (Bellman-Ford unused on hot path) | Research / deferred |
| #29 | Autonomous arbitrage bot | Research / deferred |
| #31 | Cross-chain routing | Research / deferred |

## Release gates (aspirational)

### Release 1 — “Actually swappable”

Focus: #35 (done) → #57 → #27 → #26 → #6 (done) → #33 → #53 → #25

**Exit criteria (target):** build → sign → execute on an isolated harness for supported matrices; dry-run in API; no quotes from known-stale state.

### Release 2 — “Better execution”

Focus: #30, #32, #54, #34/#55, #37, #50

**Exit criteria (target):** measured net-quote win rate and coverage on a defined basket; p95 quote SLO defined and met in tests.

### Release 3 — “Preferred product surface”

Focus: #28, #41, #42, #44, #45

**Exit criteria (target):** measurable quote→success funnel; fees and min-out visible before sign.

### Release 4 — “Operable service”

Focus: #7/#8, #19/#21/#40, #38/#39, #46

**Exit criteria (target):** load-test budgets, dependency failure drills, alerts/runbooks.

## Why someone might choose DevilRay (goals)

These are intended differentiators once the corresponding roadmap items land — **not** claims about the current release:

- Better net price across wider Sui liquidity
- Quotes validated before signing (dry-run)
- Transparent slippage, min-out, gas, and fees
- Less manual coin-object juggling
- Clear failure reasons and transaction status
- Visible token/pool risk and fresher state

## Contributing to the roadmap

Open a [feature request](../.github/ISSUE_TEMPLATE/feature_request.yml) or [DEX integration](../.github/ISSUE_TEMPLATE/dex_integration.yml) issue. Prefer linking a stable `#N` ID when updating status in PRs.
