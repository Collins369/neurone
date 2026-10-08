# Neurone — Pyth SOL/USD via Yellowstone: Investigation + Implementation

Goal: obtain a trustworthy, sufficiently fresh SOL/USD reference by subscribing
to the on-chain Pyth SOL/USD price-feed account through the **existing Solami
Yellowstone gRPC transport**, and integrate it only if proven live.

Result: **OUTCOME A — PROVEN AND INTEGRATED.** All three layers were proven
independently (feed exists; Yellowstone delivers its account; the resulting
price is accurate/fresh enough for M4's $2k/$10k boundaries), and the runtime
now sources SOL/USD from Pyth over Yellowstone. Yellowstone remains the only
realtime transport (no HTTP, no RPC polling, no extra WebSocket, no Blur).

---

## 1. Exact Pyth feed ID

`SOL/USD` = `ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d`
(from Pyth's official sponsored-feeds table for Solana).

## 2. Exact Solana price-feed account

`7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE` — a normal Solana account
(134 bytes, **owner = the Pyth receiver program**), containing a
`PriceUpdateV2` for the SOL/USD feed. This is the *sponsored* Solana push feed,
so it is maintained by Pyth's own updater.

## 3. Pyth program IDs (official docs)

| Program | Address |
|---|---|
| Receiver (pull/pro) program | `rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ` |
| Price-feed (push) program | `pythWSnswVUd12oZpeFP8e9CVaEqJg25g1Vtc2biRsT` |

`H6ARHf6YXhGYeQfUzQNGk6rDNnLBQKrenN712K4AQJEG` (owner `FsJ3A3u2…`) is the
**pre-upgrade legacy** push account; it is effectively dead (last write ≈ 25 h
before this investigation) and is NOT used.

## 4. Exact account layout (`PriceUpdateV2`, 134 bytes)

```
@0   discriminator        8   = 22 f1 23 63 9d 7e f4 cd
@8   write_authority     32
@40  verification_level   1..2 (0 = Partial{num_signatures:u8}, 1 = Full)
@41+ feed_id             32   = SOL/USD feed id (checked)
     price               i64
     conf                u64
     exponent            i32
     publish_time        i64
     prev_publish_time   i64
     ema_price           i64
     ema_conf            u64
     posted_slot         u64
```

Implemented in `src/pyth.rs` (`decode_sol_usd_price_update`), validated against a
captured real fixture (`fixtures/pyth_sol_usd_price_update_v2.hex`).

## 5. Price / exponent / confidence semantics

* `USD = price * 10^exponent`. The live feed uses `exponent = -8`.
* Internal fixed point: **micro-USD (1e-6 USD)** —
  `micros = price * 10^(exponent + 6)`, integer-only, floored when
  `exponent < -6`, overflow-checked. This matches the existing
  `sol_usd_price_micros` strategy unit exactly (no float anywhere).
* `conf` (half-width) is decoded and exposed; `verification_level` must be
  `Full`; `price <= 0`, a bad exponent, or overflow → rejected (fail closed).

## 6. Exact Yellowstone subscription

One bounded **account-address** filter in the existing subscribe request
(`[ingest.filters] sol_usd_accounts = ["7UVimffx…"]` → a named `sol_usd`
account filter). No program-wide subscription, no firehose, no dynamic
subscriptions. Account updates for that address are decoded at the ingestion
boundary and routed to the SOL/USD reference state — **never** to a market shard
(verified: no fake market is created).

## 7. Live update statistics (Solami Yellowstone)

180 s subscription to the exact account (`tests/live_pyth.rs`, ignored live test):

```
updates=3  decode_failures=0  non_target=0
first_update_after=55.2 s
slot gap between updates: 197 slots (p50)
publish_time age at receipt: min=0  p50=0  max=0
price_usd: 111.253373 → 111.521315  (+0.24% over 3 min)
```

Cross-checked on-chain: of **297** recent transactions touching the account,
exactly **1** had it writable over ~65 s — the rest were *reads* by DeFi
consumers. So the account is written ~once per minute (heartbeat), and
Yellowstone delivered every write we could observe.

## 8. Actual update cadence

* Heartbeat: documented `time_difference = 55 s`; measured ≈ 55–65 s between
  writes when the price moves < 0.5 %.
* Deviation trigger: `price_deviation = 0.5 %` — during a move the feed
  re-publishes on ≥0.5 % change (observed back-to-back publishes 1 s apart).

## 9. Actual latency

* `publish_time` age at receipt was **0 s** in every observed update → the
  on-chain write reaches Neurone within the same second it is published.
* Channel: Pyth publish → Solana account write → Yellowstone gRPC → normalize +
  decode → reference state. Delivery is sub-second; the *update cadence* is the
  heartbeat/deadband above, not the transport.
* M4's local read is an atomic load: strategy evaluation p50 stayed **100 ns**
  in the live run (no network in `evaluate`).

## 10. Freshness analysis

The published price is bounded in *value*: Pyth re-posts whenever the price
deviates by ≥0.5 %, so a stale print can never be more than ~0.5 % off until the
next heartbeat. Freshness policy (explicit, deterministic, configurable):

* `sol_usd_max_staleness_ms = 120000` (default) — measured on the **monotonic
  receive clock** (immune to wall-clock skew). It is 2× the documented 55 s
  heartbeat, i.e. one missed heartbeat of tolerance.
* Absent / stale / invalid → `read_micros()` returns `None` → M4 fails closed
  (`REFERENCE_UNAVAILABLE`, bucketed into `rejected_state`).

## 11. Independent-reference comparison

Validation only (never a runtime dependency; fetched a handful of times):

```
                    Pyth          Coinbase     diff
t0   $111.2534  (age 32 s)   $111.39   -0.123%
t1   $111.2534               $111.26   -0.006%
Kraken SOLUSD last $111.24 (same window)   ~ -0.01%
```

Deviation ≤ 0.13 %, i.e. ~$2.6 on a $2,000 boundary — far inside the 0.5 %
deviation bound and negligible for the $2k/$10k/$1k gates.

## 12. Confidence / validity analysis

* Reported confidence ±$0.03 on a ~$111 price (±0.03 %).
* Neurone requires: discriminator + feed id match, `verification_level = Full`,
  `price > 0`, exponent handled, and `publish_time` strictly newer than the last
  accepted one (duplicates/out-of-order rejected). `conf` is decoded and exposed
  but **not** used as a gate — the project has defined no confidence threshold,
  so none was invented.

## 13. Startup / reconnect / failure behavior

* **Startup without a price**: the reference is `None` → M4 evaluates and fails
  closed (`REFERENCE_UNAVAILABLE`); observed live: 154,572 such rejections in the
  first ~55 s, then normal evaluation once the first update arrived.
* **Pyth stops updating**: `received_ns` ages past 120 s → stale → fail closed.
* **Yellowstone disconnect / malformed data / invalid price / stale
  publish_time**: no state change (rejected), or the reference ages out.
* **A Pyth failure never stops ingestion**: the market engine runs on
  independent shards; only USD-dependent qualification fails closed. Verified by
  construction (the reference is a separate `Arc<SolUsdReferenceState>`; the
  Yellowstone market path is untouched) and by the live run.

## 14. Exact architecture implemented

```
Pyth SOL/USD account  →  Yellowstone gRPC (one bounded account filter)
                      →  events::normalize (decode PriceUpdateV2)
                      →  engine diverts it to SolUsdReferenceState (Arc atomics)
                      →  shard reads it LOCALLY (atomic) at evaluate time
                      →  M4
```

Files changed:

```
src/pyth.rs            (new) PriceUpdateV2 decoder + feed/account/program ids + tests
src/reference.rs       SolUsdReferenceState (lock-free atomics) + state tests
src/events.rs          AccountUpdate.pyth_sol_usd; decode at the ingestion boundary
src/engine.rs          owns the reference; diverts Pyth account updates (no shard routing)
src/shard.rs           resolves the SOL/USD price locally and passes it to evaluate()
src/strategy.rs        SolUsdSource {static|pyth_yellowstone}; sol_usd_max_staleness_ms;
                       RejectReason::ReferenceUnavailable; evaluate() takes the resolved price
src/config.rs          [ingest.filters] sol_usd_accounts (default = Pyth account) + validation
src/ingest/solami.rs   emits the bounded sol_usd account filter (+ tests)
src/telemetry.rs       sol_usd_reference_updates counter + report line
src/lib.rs, src/bench.rs, tests/*   wiring + literals + tests
config/default.toml    sol_usd_source="pyth_yellowstone", sol_usd_max_staleness_ms, sol_usd_accounts
fixtures/pyth_sol_usd_price_update_v2.hex (real captured account)
tests/live_pyth.rs     (new, ignored live probe)
docs/M4_PYTH_SOL_USD_REPORT.md
```

M4 `evaluate()` performs **no** network I/O, never waits, and holds no lock.

## 15. Performance impact

* M4 evaluation p50 **100 ns** in the live run (unchanged); the Pyth read is one
  `AtomicU64` load + a compare.
* One extra bounded account filter (a single address; ~134 bytes ≈ once per
  minute). No global scan, no per-market task, no allocation in the strategy
  path.
* Benchmarks unchanged from the M4 baseline (~250k ev/s with M4 active).

## 16. Full test results

```
cargo test --all-targets                      196 passed, 0 failed, 2 ignored
cargo fmt --check                             clean
cargo clippy --all-targets --all-features -- -D warnings   clean
cargo build --release --locked                ok
cargo test --test live_solami -- --ignored    1 passed  (#[ignore] retained)
cargo test --test live_pyth -- --ignored      1 passed  (180 s live probe)
```

New deterministic tests: Pyth decoder (real fixture, wrong disc/feed/truncation,
exponent/overflow/zero/negative/partial), reference state (unknown, fresh,
exact stale boundary, duplicate/out-of-order, invalid, deterministic reads),
strategy (`no reference → fail closed`), and engine integration (Pyth updates the
reference and creates no market; Pyth mode fails closed without a reference and
qualifies with a fresh one; static mode still works).

## 17. Final OUTCOME

`OUTCOME A — PROVEN AND INTEGRATED`

Pyth SOL/USD exists on Solana; Yellowstone delivers its account reliably
(3/3 observed writes, sub-second publish→arrival); the decoded price agrees with
independent references within 0.13 %; and a deterministic, configurable,
fail-closed freshness policy (120 s) bounds staleness. Neurone now sources
SOL/USD from Pyth over the single Yellowstone transport.

## 18. Remaining limitations

1. **Cadence, not latency.** The sponsored feed updates on a ~55–60 s heartbeat
   plus a 0.5 % deviation trigger — it is not a per-slot tick. The *value* is
   bounded (≤ ~0.5 % deviation) and refreshes fast during moves; during quiet
   periods the print can be up to ~1 minute old. This is documented, and M4
   fails closed past 120 s.
2. **Third-party updater.** Solana push updates are posted by Pyth's sponsored
   updater; if it stops, the reference goes stale and M4 fails closed (static
   mode remains available explicitly). No silent fallback.
3. **Confidence is exposed but not gated** (no project threshold defined).
4. **Static mode unchanged**: it stays available and must be selected
   explicitly; M4 does not silently fall back to it.
5. **Economic source.** Pyth is an oracle **price publisher / aggregation
   network** over first-party publisher contributions — not an exchange, not a
   liquidity pool. We rely on its published SOL/USD value only as a USD
   valuation reference; Neurone does not trade SOL/USD and makes no "deepest
   market" claim.
6. The `sol_usd_max_staleness_ms = 120000` default is a documented choice
   derived from the feed's 55 s heartbeat (2× tolerance), configurable.

**No M5/M6 functionality was introduced.** No arming, triggers, transaction
construction, signing, Beam, capital, positions, TP/SL, exits, P&L, LLM/social
analysis, HTTP polling, RPC polling, new DEX integrations, dynamic firehose,
global scans, or DB hot-path writes. M4 thresholds are unchanged ($2,000
liquidity, $2,000–$10,000 MCAP, $1,000 rolling 5m volume, 900 s max age) and
`max_drawdown_bps` remains unset/`None`.
