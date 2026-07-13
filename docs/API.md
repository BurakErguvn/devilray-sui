# API

Base URL (local default): `http://localhost:3000`

Amounts are **decimal strings** representing `u128` base units. Do not send floating-point amounts.

There is **no authentication** today. Treat the bind address as a trusted network boundary.

## REST endpoints

### `GET /health`

Liveness probe.

```json
{ "status": "ok", "service": "devilray-sui" }
```

### `GET /readyz`

Readiness when topology bootstrap finished **and** the pool catalog is non-empty.

Does not yet assert Redis/RPC/queue health (planned #19).

### `GET /api/v1/tokens`

Registered tokens (`address`, `symbol`, `name`, `decimals`). Prefer Redis `tokens:all` with PostgreSQL fallback.

### `GET /metrics`

Prometheus text exposition (`quote_*`, `discovery_*`, `dlq_pushed_total`, `active_pools_count`, …).

### `GET /api/quote`

| Query | Type | Required | Description |
| --- | --- | --- | --- |
| `from_token` | string | yes | Coin type |
| `to_token` | string | yes | Coin type |
| `amount` | string | yes | Input amount (`u128`) |

**Behavior**

- Loads active pools from Redis topology cache (PostgreSQL fallback).
- Overlays live Redis `pool:{id}` state.
- Skips paused pools.
- Returns `404` when no route exists; `503` when topology is not ready and the catalog is empty.

**Example**

```bash
curl -sG "http://localhost:3000/api/quote" \
  --data-urlencode "from_token=0x2::sui::SUI" \
  --data-urlencode "to_token=0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN" \
  --data-urlencode "amount=1000000000"
```

**Response shape**

```json
{
  "from_token": "0x2::sui::SUI",
  "to_token": "0x5d4b...::coin::COIN",
  "amount_in": "1000000000",
  "amount_out": "...",
  "price_impact": 0.0012,
  "route": [
    { "dex_name": "Cetus", "pool_address": "0x...", "weight": 100 }
  ]
}
```

### `POST /api/build_tx`

Builds an **unsigned** canonical Sui transaction.

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `from_token` | string | yes | Input coin type |
| `to_token` | string | yes | Output coin type |
| `amount` | string | yes | Input amount |
| `user_address` | string | yes | Sender / recipient address |
| `slippage_tolerance` | number | yes | Finite `f64`, `0 <= value < 1` |
| `coin_ids` | string[] | no | Explicit input coin object IDs |

**Slippage**

- Normalized to basis points at the API boundary.
- Invalid values (`<0`, `>=1`, NaN/Inf) → `400`.
- Intermediate hops use a compound budget; the last hop uses the full path minimum.
- `sqrt_price_limit` is derived from tick-aware final price when available.

**Example**

```bash
curl -sX POST "http://localhost:3000/api/build_tx" \
  -H "Content-Type: application/json" \
  -d '{
    "from_token": "0x2::sui::SUI",
    "to_token": "0x5d4b302506645c37ff133b98c4b50a5ae14841659738d6d733d59d0d217a93bf::coin::COIN",
    "amount": "1000000000",
    "user_address": "0xYOUR_ADDRESS",
    "slippage_tolerance": 0.005
  }'
```

**Response includes**

- `transaction_data_bcs` — base64 BCS of unsigned `Transaction`
- `transaction_digest` — base58 digest
- `gas_budget` / `gas_price`
- `object_refs` — resolved object summary
- `debug_transaction` — symbolic PTB for inspection
- `min_amount_out` — string `u128`

> DevilRay does **not** submit transactions. Callers must sign and execute with their own wallet / SDK after independent validation.

## WebSocket — `GET /ws`

JSON messages over a WebSocket upgrade.

**Client → server**

```json
{ "action": "subscribe", "pool_id": "0x..." }
```

```json
{ "action": "unsubscribe", "pool_id": "0x..." }
```

**Server → client**

```json
{ "type": "subscribed", "pool_id": "0x..." }
```

```json
{ "type": "pool_update", "state": { "pool_id": "0x...", "dex_name": "Cetus", "sqrt_price": "...", "liquidity": "..." } }
```

Updates originate from `DynamicPoolManager` broadcasts after Redis writes.

## Errors

| HTTP | Typical cause |
| --- | --- |
| `400` | Invalid amount, slippage, or JSON body |
| `404` | No route |
| `503` | Topology not ready / empty catalog |
| `500` / `502` | Internal or upstream metadata failures |

Error body shape:

```json
{ "error": "human readable message" }
```

## Planned endpoints

Not implemented yet (#9): `/api/v1/pools`, `/api/v1/price`, `/api/v1/routes` (debug/admin).
