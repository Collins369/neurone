# Neurone Milestone 1 — Completion Report

Scope: **Milestone 1 only** — runtime foundation + live Solami Yellowstone
ingestion + parallel sharded market-state engine + telemetry. **No live
trading.**

Readiness: **implemented, unit tested, integration tested (transport mocked),
live-endpoint connectivity verified; live event reception BLOCKED** by the
absence of a Solami credential in this environment. No claim of "live verified"
is made.

---

## 1. Files created / changed

All files are new (the workspace contained only `task.md` and
`NEURONE_BLUEPRINT.md`).

```
Cargo.toml
Cargo.lock
.gitignore
.env.example
README.md
config/default.toml
docs/SOLAMI_RESEARCH.md
docs/MILESTONE_1_REPORT.md
src/lib.rs
src/main.rs
src/error.rs
src/config.rs
src/shutdown.rs
src/clock.rs
src/hash.rs
src/events.rs
src/market.rs
src/shard.rs
src/engine.rs
src/telemetry.rs
src/ingest/mod.rs
src/ingest/solami.rs
src/ingest/simulated.rs
src/runtime.rs
src/bench.rs
tests/architecture.rs
tests/ingest_reconnect.rs
```

≈3,400 lines of Rust (src + tests).

## 2. Repository structure

```
neurone/
├── Cargo.toml / Cargo.lock
├── README.md
├── NEURONE_BLUEPRINT.md        (source of truth, unchanged)
├── task.md                     (milestone definition, unchanged)
├── .env.example
├── config/default.toml
├── docs/{SOLAMI_RESEARCH.md,MILESTONE_1_REPORT.md}
├── src/
│   ├── main.rs                 (run | check | bench)
│   ├── lib.rs
│   ├── config.rs  error.rs  shutdown.rs  clock.rs  hash.rs
│   ├── events.rs               (normalization)
│   ├── market.rs               (incremental state)
│   ├── shard.rs  engine.rs     (parallel state)
│   ├── telemetry.rs
│   ├── runtime.rs              (supervision + graceful shutdown)
│   ├── bench.rs
│   └── ingest/{mod.rs, solami.rs, simulated.rs}
└── tests/{architecture.rs, ingest_reconnect.rs}
```

This follows the blueprint's suggested layout; strategy/safety/arming/capital/
execution/position modules are intentionally absent (later milestones).

## 3. Neo Agent skills used

The Neo Agent skills at `/home/xion/neo-agent` are a development process, not a
runtime dependency. The applied ones:

* **orchestrator / PROTOCOL** — rigor classification (`critical`: this is a
  trading system's foundation), phase ordering, evidence vocabulary.
* **architecture-guardian** — invariants kept (parallel, continuous,
  deterministic, minimal hot path), explicit non-goals, bounded scope.
* **vertical-slice-builder** — each slice is runnable/testable (foundation →
  normalization → shards → ingestion → telemetry).
* **integration-provider-builder** — the Solami provider boundary: normalized
  failures, timeouts, retry/backoff, keepalive, secret-safe config,
  shutdown/cancellation, deterministic fake transport.
* **unit-test-engineer / integration-test-engineer** — unit tests plus
  integration tests over the real engine; the mock is confined to the network
  transport boundary and the report states that explicitly.
* **adversarial-test-engineer** — replay/duplicate, reordering, stale
  `write_version`, connect failure, stream interruption.
* **quality-gate** — `cargo build`, `cargo test`, `cargo clippy --all-targets`
  (clean).
* **human-code-reviewer** — small modules, explicit ownership, comments that
  explain *why*.
* **security-boundary-reviewer** — secret handling (env-only, never logged,
  never committed), fail-closed on missing/invalid credential.

## 4. Solami interfaces actually used

Full record in `docs/SOLAMI_RESEARCH.md`. Summary:

| Item | Value used |
| --- | --- |
| Endpoint | `https://grpc.solami.dev:443` (TLS) |
| Auth | `x-token` gRPC metadata = API key |
| Client | `yellowstone-grpc-client` 14.0.1 (`GeyserGrpcClient`) |
| Proto | `yellowstone-grpc-proto` 13.0.0 (`geyser` service) |
| RPC | `geyser.Geyser/Subscribe` (bidirectional stream) |
| Keepalive | `SubscribeRequest { ping }` on the same stream |
| Recovery | reconnect with `from_slot = last_seen + 1` |

## 5. Yellowstone subscription details

Built by `ingest::solami::build_subscribe_request` (`neurone check` prints it):

* `accounts` filter — `owner = [pump.fun program]` (+ optional explicit
  accounts); scoped, not a firehose.
* `txs` filter — `account_include = [pump.fun program]`, `vote = false`;
  scoped by program id.
* `slots` filter — `filter_by_commitment = false`, `interslot_updates = true`.
* `blocks_meta` filter — block boundary metadata.
* `commitment = PROCESSED` (configurable: processed | confirmed | finalized).
* `from_slot` set on reconnect.

Program id used by default: `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P`
(pump.fun), taken verbatim from Solami's official gRPC SDK example. All filter
addresses are configurable; program ids are validated as 32-byte base58 at
startup.

## 6. Event types implemented

Normalized, owned, bounded types (no protobuf downstream):

* `Slot { slot, parent, status }`
* `Account { pubkey, slot, owner, lamports, data_len, data_digest, write_version, is_startup, txn_signature }`
* `Transaction { signature, slot, index, is_vote, success, keys[<=64] }`
* `BlockMeta { slot, block_height, block_time, executed_transaction_count }`

`ping`/`pong` → counted as keepalive; `block`/`entry`/`transaction_status` →
counted as invalid (not subscribed).

Market state (`market.rs`) models only what is genuinely derivable today:
identity, owner, last slot, lamports, data length, data digest,
`write_version`, update counters, transaction count, last signature, status.
Price/liquidity/volume require AMM decoding and are deferred (documented, not
invented).

## 7. Parallel-state architecture

```text
normalized event ──► hash(market key) ──► shard[0..N] ──► owned market map
```

* **Deterministic routing:** `shard = fnv1a_64(pubkey) % num_shards` (fixed
  hash, never `std`'s randomised hasher).
* **Per-market ordering:** the router is a single task; every event for a key
  is sent to that key's shard in arrival order; each shard is a FIFO consumer.
* **No global lock:** each `Shard` owns its `HashMap<MarketKey, MarketState>`.
* **Independent tasks:** one tokio task per shard; a transaction touching keys
  in several shards is grouped per shard and delivered once per shard.
* **Chain-global events** (slot/block-meta) are broadcast so each shard keeps a
  local slot watermark.
* **Replay protection:** per-market `write_version` monotonicity for accounts;
  a bounded per-shard signature ring for transactions.
* **Bounded channels** (`shard_channel_capacity`) provide backpressure without
  dropping events.

The architectural-parallelism claim is proven by
`engine::tests::shards_run_as_independent_tasks`: all shards must rendezvous on
a barrier concurrently — a serial implementation would deadlock on the first
shard.

## 8. Tests and results

`cargo test` — **30 passed, 0 failed** (0.6 s):

* `src` unit tests (21): config validation, hash stability, normalization
  determinism, slot mapping, bounded/deduped tx keys, keepalive, market
  stale/duplicate rejection, tx replay idempotence, signature ring eviction,
  shutdown, telemetry counters/percentiles, simulated-source determinism,
  subscription-filter shape, **shard concurrency barrier**.
* `tests/architecture.rs` (8): identical sequences → identical state; shard
  routing deterministic + balanced; per-market ordering preserved under
  interleaving; replayed events don't corrupt state; 512 independent markets
  each advance; transactions don't create markets; global events reach every
  shard; clean shutdown.
* `tests/ingest_reconnect.rs` (1): connect failures + stream interruptions →
  reconnect, re-normalize, route, with telemetry accounting.

`cargo clippy --all-targets` — **clean (0 warnings)**.

Evidence levels: `unit tested` (logic), `integration tested` over the real
in-process engine; the Solami **transport** is the only mocked boundary, so
live-provider reception is **not** claimed.

## 9. Measured throughput and latency

Host: 1 vCPU (Ubuntu 26.04), tokio multi-thread (2 worker threads configured
minimum), 8 shards, deterministic synthetic source.

| Mode | Events | Throughput | End-to-end p50/p95/p99 | Processing p50/p99 |
| --- | --- | --- | --- | --- |
| Burst | 200,000 | **75,753 ev/s** | queue-dominated | **5 µs / 5 µs** |
| Steady 10k/s | 30,000 | 9,996 ev/s | **250 µs / 500 µs / 500 µs** | **5 µs / 10 µs** |
| Steady 50k/s | 50,000 | 49,885 ev/s | 500 µs / 5 ms / 5 ms | 5 µs / 5 µs |

Shard activity is evenly distributed (e.g. 8 × ~14.9 k for a 100 k-run).
"End-to-end" = arrival → state update (includes shard queueing); "processing" =
dequeue → applied (hot-path service time). At 10 k ev/s the hot path is ~5 µs
with sub-millisecond end-to-end latency; the burst figures are dominated by an
artificially unbounded producer, not shard service time.

The telemetry also tracks, for later milestones, the timeline
`event timestamp → state update` (`server_created_at` + `arrival_ns` are already
carried on every event).

## 10. Reconnect behavior

* Missing/invalid credential → fail closed, exponential backoff
  (500 → 1000 → 2000 → 4000 … capped by `reconnect_max_ms`), no events routed.
* Stream end / gRPC error / stale stream (no update within
  `stale_stream_timeout_ms`) → reconnect, resuming with `from_slot = last + 1`.
* If a `from_slot` subscribe is rejected (gap beyond Solami's ~3,500-slot replay
  window), the watermark is dropped and the next attempt resyncs live.
* Verified by `ingestion_recovers_from_connect_failures_and_stream_interruptions`
  (2 connect failures, then repeated stream interruptions).

Live confirmation against the real endpoint: invalid token →
`UNAUTHENTICATED: invalid api key`, then backoff and retry (logs in
`docs/SOLAMI_RESEARCH.md` §11).

## 11. Unresolved assumptions / issues

1. **No Solami credential in this environment** (only a Helius key exists
   elsewhere on the host). Live event reception is therefore **not verified**;
   everything up to and including authentication was verified against the real
   endpoint.
2. Default program filter is pump.fun only (from Solami's docs). Adding more
   AMM programs is configuration, not code.
3. `data_digest` is a fingerprint, not decoded AMM state. Price/liquidity/volume
   need instruction/account decoders (a later milestone).
4. `slots_observed`/`latest_slot` are updated by the ingestion path; shard
   watermarks are separate. Tests assert the shard watermark.
5. Per-shard transaction dedup is a bounded ring (8,192 signatures); a duplicate
   older than the ring could be re-applied. Per-market `write_version` still
   protects account state; this is a documented bound.
6. Backpressure is queueing-based (`send().await`); under sustained overload a
   slow shard applies backpressure to the router. Acceptable at M1 volumes;
   revisit with per-shard load shedding if needed.

## 12. Recommended next milestone

**Milestone 2 — decode real market state from the stream.** Add AMM
account/instruction decoding (pump.fun bonding curve, then one AMM) so
`MarketState` carries executable reserves/liquidity/price and per-slot volume,
keeping the same sharded architecture. This is the prerequisite for the
deterministic filter/qualification stage and produces the fields the blueprint's
strategy needs — before any arming, capital or Beam work.

---

## Stop condition

Per task §14, work stops here. No trading, transaction construction, signing,
Beam, TP/SL, capital arbitration or strategy logic was implemented.
