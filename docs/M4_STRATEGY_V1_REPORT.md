# Neurone — M4 Strategy V1 Report

## M4 status

`IMPLEMENTED — deterministic OBSERVING → QUALIFIED, validated live.`

M4 converts continuously maintained M3 market state into a deterministic
qualification decision and stops there. No arming, triggering, transaction
construction, signing, submission, capital, positions, or TP/SL.

---

## Files changed

```
src/strategy.rs        (new)   StrategyConfig, RejectReason, Decision, evaluate()
src/market.rs          MarketStatus::Qualified; created_at_slot / peak_mcap_quote /
                       strategy_version / last_qualified_version / consumed_version;
                       reference_price_raw / market_cap_quote / liquidity_quote /
                       rolling_quote_volume / note_mcap_peak
src/config.rs          Config.strategy: StrategyConfig
src/engine.rs          Engine::start_with_strategy(strategy, reserve_stale_slots)
src/shard.rs           per-shard M4 evaluation after each applied state change
src/telemetry.rs       M4 counters + evaluation-latency histogram + report line
src/runtime.rs         wires config.strategy + config.market into the engine
src/bench.rs           bench::run uses the configured strategy; strategy_microbench()
src/main.rs            `bench` prints the strategy microbench
src/lib.rs             pub mod strategy
config/default.toml    [strategy] section (locked values; operator price input)
tests/strategy.rs      (new) 34 deterministic tests
```

No change to `src/quote.rs`, `src/decode/*`, `src/validate.rs` semantics, or the
sharding/ingestion architecture.

---

## Architecture changes

None structural. The hot path is unchanged in shape:

```
Yellowstone event → normalize → hash(pool/mint) → owning shard
   → update one MarketState → evaluate M4 for that market → QUALIFIED / OBSERVING
```

Evaluation happens only for markets a shard actually updated (account updates;
decoded swaps; creates; PumpSwap vault-balance boots; invalidations) — there is
no global scan, no per-market task, no lock, no blocking I/O, and no allocation
in `evaluate`. Independent markets remain independent (tested).

---

## Strategy gates

Evaluation order (documented; the first failure is the reported reason):

1. **Supported market** — venue ∈ {pump.fun, PumpSwap}; SOL-valued quote asset
   (`None`, wrapped SOL, or the default pubkey). Non-SOL quotes →
   `UNSUPPORTED_MARKET`.
2. **State validity** — `reserve_state` must be `Known`; `Unknown`/`Stale`/
   `Invalidated` → `STATE_UNKNOWN`/`STATE_STALE`/`STATE_INVALIDATED`.
3. **Freshness** (only if `max_token_age_seconds` set) — requires chain-proven
   creation; unknown age → `INSUFFICIENT_FRESHNESS_DATA`; too old →
   `TOKEN_TOO_OLD`.
4. **Structural safety** (see below).
5. **Liquidity** — `min_liquidity_usd = 2000` inclusive.
6. **Market cap** — `2000 ≤ MCAP ≤ 10000` inclusive.
7. **Anti-pump/dump** (only if `max_drawdown_bps` set) — peak-MCAP retracement.
8. **Rolling 5-minute volume** — `min_rolling_volume_usd = 1000` inclusive.
9. **Buy route** — deterministic executable buy quote from current state.
10. **Sell route** — deterministic reverse sell of the bought tokens.
11. **Execution economics** (only if configured) — price-impact / round-trip loss.
12. **Dedup** — a consumed opportunity at the current strategy version cannot
    re-qualify (`ALREADY_CONSUMED`).

**Deterministic monetary model.** Thresholds are USD. There is no live oracle in
scope, so USD is computed from a configured static price
(`sol_usd_price_micros`): `usd_micros(quote_raw) = quote_raw × price_micros /
1e9` (SOL/lamports). MCAP = `reference_price × token_total_supply` in raw quote
units; liquidity = `2 × quote-side depth`. Both are documented deterministic
choices because the project defined neither formula; the M2 report's "liquidity
= protocol reserves" is honoured via the real quote reserve for pump.swap and
the curve depth for pump.fun. All arithmetic is integer; no floats in decisions.

**Rolling volume.** Reuses the bounded per-slot `VolumeWindow`
(`totals_since(from_slot)`), window slots = `ceil(seconds × 2.5)`
(`300 s → 750 slots`); a bucket is in-window iff `age_slots ≤ window_slots`;
expiration is exact at the boundary. Amortized O(1) update, O(1) query over
retained buckets, bounded memory (1024 buckets ≈ the 750-slot window).

---

## Safety checks

Implemented (deterministic, provable from current Neurone data):

* supported venue + valvable quote asset;
* `Known`/current reserves (no unknown/stale/invalidated state);
* pump.fun: curve not completed/migrated; non-zero token supply; positive virtual
  base/quote reserves;
* PumpSwap: base/quote mints present; non-zero base/quote reserves;
* executable buy **and** reverse sell quotes with non-zero outputs;
* optional price-impact and round-trip-loss bounds.

Explicitly **not** claimed (not observable with the current ingestion — no
mint-account stream): mint authority, freeze authority, Token-2022 extensions,
and metadata. Unknown information fails closed where it is *required* by a gate;
these are documented limitations, not guarantees. M4 never claims to predict a
rug.

---

## Configuration

```toml
[strategy]
min_liquidity_usd = 2000
min_market_cap_usd = 2000
max_market_cap_usd = 10000
rolling_volume_window_seconds = 300
min_rolling_volume_usd = 1000
# OPERATOR INPUT (no invented default). Without it M4 is inactive.
# sol_usd_price_micros = 200000000
# probe_notional_lamports = 100000000   # 0.1 SOL
# max_token_age_seconds / max_drawdown_bps / max_roundtrip_loss_bps /
# max_price_impact_bps  — all unset by default (no project value specified).
```

The locked thresholds are exactly the V1 spec. Nothing numeric is invented:
the price is an explicit operator input, and freshness/drawdown/economics stay
`None` (gate disabled) unless set. No volume-acceleration configuration exists.

---

## Tests

`tests/strategy.rs` — 34 deterministic tests:

* **Liquidity** 1999/2000/2001; **MCAP** 1999/2000/10000/10001.
* **Volume** 999/1000/1001; multi-trade accumulation; 2-minute token with $1000+
  passes; expired volume removed; exact window boundary (age 750 in, 751 out);
  zero/sparse volume; high trade count; bounded structure.
* **Freshness** new token; too old; unknown creation fails closed (and is not
  inferred from first observation); drawdown requires chain-proven history;
  pumped-then-dumped rejected; acceptable drawdown passes.
* **Safety** completed curve; zero supply; non-SOL quote; PumpSwap missing mints.
* **State** Unknown/Stale/Invalidated; recovery after authoritative update.
* **Buy/sell** unavailable when the probe yields nothing / cannot be reversed;
  both routes available for a qualifying market.
* **Economics** generous bounds pass; zero impact bound fails; exact round-trip
  loss boundary passes and one-less fails.
* **Dedup** consumed cannot re-qualify; temporary failure qualifies after a state
  change.
* **Engine integration** a market qualifies and is counted once; independent
  markets do not contaminate; recovery re-qualifies; inactive strategy is a no-op.
* **Determinism** identical state → identical decision (1000×).

Full suite: `cargo test --all-targets` → **160 passed, 0 failed, 1 ignored**.

---

## Benchmarks

`cargo run --release -- bench` (`NEURONE_BENCH_EVENTS=200000`, 8 shards,
2 workers), M4 active vs inactive (same synthetic load):

| Metric | M4 active | M4 inactive |
|---|---:|---:|
| throughput (events/s) | 269,730 | 295,495 |
| state updates | 318,405 | 318,405 |

Strategy microbench: `ns_per_eval ≈ 217` (p50 201 ns, p95 207 ns, p99 317 ns);
rolling-volume update ≈ 650–960 ns. M4 adds ~9% throughput cost in this
worst-case synthetic (every event triggers an evaluation); live ingestion runs
at ~700–1,500 events/s, where this is negligible. Evaluation is allocation-free.

---

## Live validation

`NEURONE_CONFIG=<config with sol_usd_price_micros = 200000000> neurone run`
(45 s, Solami, release):

```
markets_evaluated       84,439
markets_qualified           42
rejected_unsupported     9,485
rejected_low_liquidity     814
rejected_low_5m_volume   1,512
rejected_mcap            1,050
rejected_safety         69,483   (mostly completed/migrated curves + non-SOL)
rejected_state             349
rejected_buy/sell/execution/consumed/too_old/already_pumped   0
strategy_eval p50=100ns p99=5000ns
active_market_states     3,934
```

Real market state flows through the gates and 42 markets newly qualified (the
inclusive thresholds behave as specified). No transactions were sent.

Regression checks: `live_solami` passed; `neurone validate` (M3 parity) shows
**0 mismatches** on all supported paths. (One degenerate real swap sample with a
~empty pool is counted as a harness `ZeroReserves` error — a pre-existing,
data-dependent classification in the untouched validator, not an M4 regression.)

---

## Prerequisite fixes

None required. Verified:

* `TransactionUpdate.success` **is** consumed by the shard (M3 fix) — failed
  transactions do not mutate state, and M4 evaluation only runs for applied,
  successful state (`src/shard.rs`).
* Pre-state corroboration (`UnsupportedState`, `previous_event_not_contiguous`)
  exists only in `src/validate.rs`; the runtime does not gate on it (see
  `LATE_MARKET_STATE_SEMANTICS_REPORT.md`). M4 therefore evaluates markets
  discovered after launch normally.

---

## Known limitations

1. **Static USD price.** No live oracle is in scope, so USD thresholds use a
   configured constant and drift with the real SOL price. Non-SOL-quoted markets
   are rejected (`UNSUPPORTED_MARKET`) rather than mis-valued.
2. **MCAP/liquidity formulas** are documented deterministic choices (the project
   defined neither).
3. **Freshness & anti-pump require full-history observation** (chain-proven
   creation). When either gate is enabled, markets whose creation was not
   observed fail closed (`INSUFFICIENT_FRESHNESS_DATA`) — a deliberate
   consequence of "do not equate first observation with creation".
4. **Mint/freeze authority and Token-2022 extensions are not observable** (no
   mint-account data); they are not claimed.
5. **Rolling window is slot-based** (400 ms slot assumption) and bounded to 1024
   buckets, which comfortably holds the 750-slot default window.
6. **M4 is inactive until `sol_usd_price_micros` is set** — deliberate (no
   invented price).

---

## Intentionally unimplemented items

Volume acceleration; arming/pre-arming; entry triggers; transaction
construction/signing/submission/Beam; capital arbitration; position management;
TP/SL; exits; P&L; LLM/social/narrative analysis; DexScreener/API polling; RPC
polling; DB hot-path persistence; frontend; ShredDirect; Blur; Mirage; Index
Engine; webhooks; dynamic token-account firehose.

---

## Verdict

`M4 STRATEGY V1 COMPLETE (qualification only).`

Deterministic, fail-closed, parallel (shard-local), bounded, and measured:
thresholds exactly match the V1 spec (including inclusive boundaries and the
2-minute-token case), unknown/stale/invalidated and unprovable conditions cannot
qualify, both buy and sell routes are required, and consumed opportunities are
deduplicated. No trading path was added.
