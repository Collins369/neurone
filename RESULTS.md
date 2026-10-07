# Neurone — Milestone 1 Results

**Date:** 2026-10-07
**Scope:** Milestone 1 only — runtime foundation + live Solami Yellowstone
ingestion + parallel sharded market-state engine + telemetry. **No live
trading.**
**Repository:** `/home/xion/neurone` (git, branch `main`, commits `9845e92`,
`07de579`)

> Readiness: **implemented, unit tested, integration tested (transport
> mocked), live-endpoint connectivity verified.**
> **Live event reception: BLOCKED** — no Solami credential exists in this
> environment. No "live verified" claim is made.

---

## 1. Deliverable checklist (task §13)

| # | Requirement | Status |
|---|---|---|
| 1 | Builds successfully | ✅ `cargo build` |
| 2 | Passes its tests | ✅ 30/30 |
| 3 | Connects to Solami Yellowstone via the real configured interface | ✅ verified (TLS + `x-token`), auth rejected only due to missing key |
| 4 | Receives live events | ⛔ **not verified** — no credential available |
| 5 | Normalizes relevant events | ✅ unit tested |
| 6 | Routes market events through deterministic shards | ✅ tested |
| 7 | Maintains multiple market states concurrently | ✅ tested (512 markets) |
| 8 | Exposes useful runtime telemetry | ✅ counters + latency histograms |
| 9 | Handles disconnect/reconnect safely | ✅ tested + live backoff observed |
| 10 | Shuts down cleanly | ✅ tested |
| 11 | Demonstrates the architecture is parallel, not serial | ✅ barrier test |

## 2. What was built

```text
Solami Yellowstone gRPC  (x-token auth, ping keepalive, from_slot replay)
        │
        ▼
normalization  (compact owned events; no protobuf downstream)
        │
        ▼
parallel sharded engine  (fnv1a(key) % shards -> owned market map)
        │
        ▼
lock-free telemetry  (atomics + latency histograms)
```

* Deterministic shard routing: `fnv1a_64(pubkey) % num_shards` (fixed hash,
  never `std`'s randomised hasher).
* One independent tokio task per shard; no global lock over market state.
* Per-market ordering preserved (single router task → per-shard FIFO channel).
* Replay protection: per-market `write_version` monotonicity + bounded per-shard
  signature ring.
* Bounded channels for backpressure; cooperative shutdown that drains.
* Secrets env-only (`SOLAMI_GRPC_TOKEN`), never logged, never committed.

## 3. Solami interface actually used

Full record with sources: `docs/SOLAMI_RESEARCH.md`.

| Item | Value |
|---|---|
| Endpoint | `https://grpc.solami.dev:443` (TLS) |
| Auth | `x-token` gRPC metadata = API key |
| RPC | `geyser.Geyser/Subscribe` (bidirectional stream) |
| Client | `yellowstone-grpc-client` 14.0.1 (`GeyserGrpcClient`) |
| Proto | `yellowstone-grpc-proto` 13.0.0 |
| Keepalive | `SubscribeRequest { ping }` every 15 s |
| Recovery | reconnect with `from_slot = last_seen + 1` |
| Commitment | `PROCESSED` (configurable) |

Install Filters: `accounts` (owner-scoped), `txs` (`account_include`, not a
firehose), `slots`, `blocks_meta`. Default program id
`6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P` (pump.fun), taken from Solami's
official gRPC SDK example.

Event types implemented: `Slot`, `Account`, `Transaction`, `BlockMeta`
(`ping`/`pong` = keepalive; `block`/`entry`/`transaction_status` = not
subscribed).

## 4. Test evidence

`cargo test` → **30 passed, 0 failed** (~0.6 s)

* **21 unit tests** — config validation, hash stability, normalization
  determinism, slot mapping, bounded/deduped tx keys, market stale/duplicate
  rejection, tx replay idempotence, signature-ring eviction, shutdown,
  telemetry counters/percentiles, simulated-source determinism, subscription
  shape, **shard-concurrency barrier**.
* **8 architecture tests** — identical sequences → identical state; shard
  routing deterministic + balanced; per-market ordering under interleaving;
  replayed events don't corrupt state; 512 independent markets advance;
  transactions don't create markets; global events reach every shard; clean
  shutdown.
* **1 reconnect test** — 2 connect failures + repeated stream interruptions →
  reconnect, re-normalize, route, with telemetry accounting.

`cargo clippy --all-targets` → **clean (0 warnings)**.
`cargo fmt --check` → **clean**.

**Parallelism proof:** `engine::tests::shards_run_as_independent_tasks` makes
every shard rendezvous on a barrier concurrently. A serial implementation would
deadlock on the first shard, so passing this test proves the shards are
independent tasks.

> Evidence level: `unit tested`; `integration tested` over the real in-process
> engine. The Solami **network transport** is the only mocked boundary, so
> live-provider reception is **not** claimed.

## 5. Performance (host: 1 vCPU, 8 shards, synthetic source)

```bash
NEURONE_BENCH_EVENTS=200000 cargo run -- bench
NEURONE_BENCH_EVENTS=30000 NEURONE_BENCH_RATE=10000 cargo run -- bench
```

| Mode | Events | Throughput | End-to-end p50 / p95 / p99 | Processing p50 / p99 |
|---|---|---|---|---|
| Burst (flat out) | 200,000 | **75,753 ev/s** | queue-dominated | **5 µs / 5 µs** |
| Steady 10k/s | 30,000 | 9,996 ev/s | **250 µs / 500 µs / 500 µs** | **5 µs / 10 µs** |
| Steady 50k/s | 50,000 | 49,885 ev/s | 500 µs / 5 ms / 5 ms | 5 µs / 5 µs |

Shard activity is evenly distributed (e.g. 8 × ~14.9 k for a 100 k run).
End-to-end = arrival → state update (includes shard queueing); processing =
dequeue → applied (hot-path service time). At a realistic 10k ev/s the hot path
is ~5 µs with sub-millisecond end-to-end latency; the burst figures are
dominated by an artificially unbounded producer.

## 6. Live connectivity probe

From the build host on 2026-10-07:

* DNS: `grpc.solami.dev` → `181.215.23.23`; TCP 443 open.
* TLS verified; HTTP/2 negotiated (`curl --http2` → `200`, `http_version=2`,
  `ssl_verify_result=0`).
* Real client run with `SOLAMI_GRPC_TOKEN=invalid-test-token`:

```text
connected to Solami Yellowstone (endpoint=https://grpc.solami.dev:443)
subscribe failed: gRPC status: 'The request does not have valid authentication
  credentials', message: "invalid api key"
reconnecting ... backoff_ms=500 → 1000 → 2000 → 4000
```

This proves the endpoint, TLS, HTTP/2, `x-token` auth path, error handling and
exponential-backoff reconnect logic are correct. It also confirms **fail
closed**: no events are routed while unauthenticated.

## 7. Unresolved assumptions / issues

1. **No Solami credential in this environment** (only a Helius key exists
   elsewhere on the host). Live event reception is therefore not verified.
2. Default program filter is pump.fun only; adding more AMM programs is
   configuration, not code.
3. `data_digest` is a fingerprint, not decoded AMM state — price/liquidity/
   volume need account/instruction decoders (next milestone).
4. Per-shard tx dedup is a bounded ring (8,192 signatures); older duplicates
   could be re-applied, but per-market `write_version` still protects account
   state.
5. Backpressure is queueing-based (`send().await`); under sustained overload a
   slow shard applies backpressure to the router. Fine at M1 volumes.

## 8. Recommended next milestone

**Milestone 2 — decode real market state.** Add AMM account/instruction
decoding (pump.fun bonding curve, then one AMM) so `MarketState` carries
executable reserves/liquidity/price and per-slot volume, keeping the same
sharded architecture. This is the prerequisite for the deterministic
filter/qualification stage — before any arming, capital or Beam work.

## 9. Stop condition

Per task §14, work stops here. No trading, transaction construction, signing,
Beam, TP/SL, capital arbitration or strategy logic was implemented.

---

Detailed engineering report: [`docs/MILESTONE_1_REPORT.md`](docs/MILESTONE_1_REPORT.md)
· Solami interface record: [`docs/SOLAMI_RESEARCH.md`](docs/SOLAMI_RESEARCH.md)
