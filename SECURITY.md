# Security Policy

## Supported versions

| Version | Supported |
| --- | --- |
| `0.1.x` | Best-effort on `main` |

There is no production release line yet.

## Reporting a vulnerability

Please use **GitHub Security Advisories** for this repository (private report) rather than opening a public issue for security-sensitive findings.

Include:

- Affected component / endpoint
- Reproduction steps
- Impact assessment
- Any suggested fix (optional)

We will acknowledge reports as capacity allows.

## Known limitations (user risk, not necessarily CVEs)

DevilRay is experimental. The following are intentional gaps in the current design:

| Area | Current behavior | Risk |
| --- | --- | --- |
| Authentication | None on HTTP/WS | Anyone who can reach the bind address can quote/build |
| Rate limiting | None | DoS / RPC cost amplification |
| Transaction execution | Unsigned BCS only; no `executeTransactionBlock` in the API | Callers must validate and submit carefully |
| Dry-run | Not in API | Invalid PTBs may only fail at wallet/submit time |
| Quote freshness | No hard stale-state reject | Quotes may use aged pool/tick data |
| Tick window | Fixed ±200 style window for many fetches | Large swaps may be simulated optimistically |
| Token safety | Routes by type string | No scam/deny list |
| Metrics | Public `/metrics` | May leak operational signals |

Do **not** expose DevilRay to the public internet without a reverse proxy, auth, TLS, and network controls.

## Dependency and secret hygiene

- Never commit `.env` or private keys.
- Prefer `cargo audit` / Dependabot before relying on new crates in production settings.
- Default RPC/GraphQL/WS URLs are public mainnet endpoints — treat them as untrusted remote services.