# Operations

## Docker Compose

Full stack (`app` + PostgreSQL + Redis + ClickHouse):

```bash
cp .env.example .env
docker compose up --build
```

Test infra only (CI / local integration):

```bash
docker compose -f docker-compose.test.yml up -d --wait test-postgres test-redis
```

Container names use the `devilray-*` prefix. Dev credentials in compose are for local use only.

## Environment variables

See [`.env.example`](../.env.example).

| Variable | Notes |
| --- | --- |
| `DATABASE_URL` | Required |
| `REDIS_URL` | Required |
| `CLICKHOUSE_URL` | Optional; mock analytics when unset |
| `RPC_URL` / `GRAPHQL_URL` / `WEBSOCKET_URL` | Default to public mainnet endpoints |
| `BIND_ADDR` | Default `0.0.0.0:3000` |
| `RUST_LOG` | Default `devilray_sui=info` |
| `STATIC_SCAN_INTERVAL_SECS` | Static discovery loop interval |
| `DISCOVERY_PAGE_SIZE` / `DISCOVERY_MAX_PAGES` / `DISCOVERY_INTER_PAGE_MS` | GraphQL pacing |

## Health endpoints

| Path | Meaning |
| --- | --- |
| `/health` | Process up |
| `/readyz` | Topology ready **and** pool catalog non-empty |

`/readyz` is not a full dependency health check yet (#19).

## Metrics

Scrape `GET /metrics` (Prometheus text format). Notable series:

- `quote_requests_total{status}`
- `quote_latency_seconds`
- `active_pools_count`
- `dlq_pushed_total`
- `discovery_pools_discovered_total`
- `discovery_events_scanned_total`
- `discovery_scan_failures_total`
- `discovery_checkpoint_sequence`

Restrict `/metrics` in any shared environment.

## Dead-letter queue

`DatabaseWriter` retries failed persistence, then pushes a `DlqEntry` to `{queue}_dlq`.

Default queue name: `devilray_write_queue`.

Replay:

```bash
cargo run --bin replay_dlq -- [limit] [queue]
```

## Production notes

DevilRay is **not** production-hardened:

- No auth / rate limits / CORS policy
- Public metrics endpoint
- Mainnet RPC defaults can rate-limit or change
- Transaction path stops at unsigned BCS

If you deploy experimentally, put TLS + auth + egress controls in front of the process and never commit real secrets.
