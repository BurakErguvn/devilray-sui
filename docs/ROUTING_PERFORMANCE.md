# Routing performance (pathfinding micro-benchmarks)

These numbers measure **in-process pathfinding** on synthetic graphs. They are **not** end-to-end quote latency. Real request time is dominated by RPC, cache freshness, tick loading, and simulation — not Dijkstra/DFS alone.

Run locally:

```bash
cargo run --bin benchmark_routing
```

## Single-route algorithms

### Dijkstra (preferred single path)

| Topology | Duration |
| --- | --- |
| Small (10 tokens / 20 pools) | ~28.1 µs |
| Medium (50 / 150) | ~29.5 µs |
| Large (100 / 400) | ~38.1 µs |

- Time complexity: `O(E + V log V)`
- Memory: `O(V + E)`
- Stops when the destination is reached (binary heap priority queue)

### Bellman-Ford (cycle / arbitrage detection)

| Topology | Duration |
| --- | --- |
| Small | ~107.8 µs |
| Medium | ~842.7 µs |
| Large | ~2.75 ms |

- Time complexity: `O(V · E)`
- Supports negative edge weights / negative-cycle detection
- Not used on the default hot quote path

## Multi-route search (hop-limited DFS)

Used to enumerate candidates for convex split routing.

### Max hops H = 2

| Topology | Duration |
| --- | --- |
| Small | ~19.4 µs |
| Medium | ~28.3 µs |
| Large | ~43.9 µs |

### Max hops H = 3

| Topology | Duration |
| --- | --- |
| Small | ~42.0 µs |
| Medium | ~109.4 µs |
| Large | ~252.8 µs |

### Max hops H = 4

| Topology | Duration |
| --- | --- |
| Small | ~87.9 µs |
| Medium | ~439.2 µs |
| Large | ~1.46 ms |

Practical hop limits of H = 3–4 keep enumeration under a few hundred microseconds on these synthetic sizes.

## Interpretation

- Keep these benchmarks for algorithm regressions.
- Do not advertise them as production quote SLOs.
- Roadmap items for real latency and net-output quality include multi-RPC (#37), freshness guards (#33), and competitor shadow quoting (#50).
