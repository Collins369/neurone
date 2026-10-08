# Neurone — M5 Pre-Arming Report

M5 turns a deterministic M4-qualified opportunity into a bounded, validated
`ARMED` execution context **before** the trigger. M5 does **not** execute.
Lifecycle: `OBSERVING -> QUALIFIED -> ARMED` (M6 begins at the trigger).

## 1. Architecture audit

`MarketState` is continuously maintained; shards own disjoint maps (no global
lock, no per-market task), routed by `fnv1a(key) % N`. `MarketStatus` was
`{Observing, Qualified}` and the shard called `strategy::evaluate` after each
applied state change. Existing versioning: `state_version`,
`strategy_version` (bumped on every evaluation-relevant change),
`last_qualified_version`, `consumed_version`. M4 is the qualification gate
(venue/SOL-quote, Known state, <=900 s age, structural safety, liquidity >=
$2,000, MCAP $2k-$10k, rolling 5m volume >= $1,000, executable buy+sell routes,
economics, dedup; SOL/USD from the Pyth reference state or the explicit static
price).

Conclusion: M5 belongs **inside the owning shard, right after
`strategy::evaluate`** — extend the existing lifecycle, no second state machine.

## 2. Exact M5 transition

`Decision::Qualified -> arm/refresh -> MarketStatus::Armed`;
`Decision::Rejected -> disarm -> MarketStatus::Observing`. M4's gates *are* the
arming preconditions, so arming = "M4 currently qualifies".

## 3. Exact arming conditions

Re-verified by `strategy::evaluate` at arm time (never remembered): supported
market + SOL quote; Known/current state; trustworthy reserves; executable buy
route; executable reverse sell route; valid economics; valid SOL/USD reference;
not consumed; not already armed at the same `strategy_version` (idempotent).
Unknown fails closed.

## 4. Exact ARMED structure

```rust
pub struct ArmedContext {
    pub side: Side,                  // planned entry side (V1 = Buy)
    pub armed_slot: u64,             // first arm slot (expiry anchor)
    pub armed_ns: u64,               // monotonic ns at first arm (diagnostics)
    pub market_version: u64,         // strategy_version at arm time
    pub sol_usd_micros: Option<u64>, // reference used at arm (informational)
}
```

Immutable-data-only: no transaction, no signed bytes, no frozen reserve
snapshot (it would be stale immediately).

## 5. Exact invalidation conditions

Checked on every update to that market (shard-local): (a) any M4 rejection
(state unknown/stale/invalidated, reserve mutation, MCAP/liquidity/volume/age
out of bounds, route/economics failure, stale SOL/USD, consumed) -> context
dropped (`arm_invalidated`); (b) arm expiry -> dropped (`arm_expired`) then
re-armed if still qualified; (c) version change while still qualified ->
refreshed in place (`arm_refreshed`) keeping the original `armed_slot`, so an
active market does not churn `ARMED->QUALIFIED->ARMED`.

## 6. Exact expiry semantics

`max_arm_age_slots` (config, default **150 slots ~ 60 s**) == the existing
`market.reserve_stale_slots`: an armed context must not outlive the
reserve-freshness horizon it was armed against. Slot-exact boundary: `age <=
150` valid, `151` expired (tested). Expiry fails closed.

## 7. Exact versioning semantics

No new version system. `ArmedContext.market_version = strategy_version`: same ->
already armed (idempotent); newer while still qualified -> refresh in place;
any M4 rejection -> drop. `consumed_version` still gates qualification.

## 8. What is precomputed

The deterministic arm decision (all M4 gates re-verified) and the execution
context (side, arm slot/ns, market version, SOL/USD used). Nothing mutable.

## 9. What M6 must refresh

Before firing, M6 must: re-read state and require `reserve_state == Known`;
re-read SOL/USD and require freshness; re-quote buy/sell from current reserves;
check `armed.market_version == market.strategy_version`; check
`!armed.is_expired(...)`; and build/sign/submit from *fresh* data (reserves,
fees, blockhash, compute budget, slippage).

## 10. Trigger interface handed to M6

`market.armed` + the existing `reserve_state()` + a version compare + the
`is_expired` helper, all local to the owning shard. The trigger **condition is
not implemented** (the blueprint does not define it); no strategy logic was
invented.

## 11. Multiple armed markets

Multiple markets arm simultaneously (verified). Arming is per-market and
shard-local; one market's arm/refresh/invalidation cannot affect another
(verified).

## 12. Capital arbitration interaction

M5 does **not** arbitrate capital. The V1 one-active-position policy belongs to
M6, which selects one armed+triggered opportunity; M5 just guarantees every
armed context is individually safe and versioned.

## 13. Telemetry

New: `markets_armed`, `arm_refreshed`, `arm_already_armed`, `arm_invalidated`,
`arm_expired` (+ report line). No per-event logging.

## 14. Deterministic test results

`tests/arming.rs` (15 tests): qualified->armed; unqualified/unknown not armed;
missing SOL/USD reference not armed; stale SOL/USD disarms; reserve invalidation
disarms; MCAP leaving bounds disarms; reserves leaving bounds disarms; token age
>15 min disarms; version change refreshes without duplicate arm; expiry drops
then re-arms; invalidated market requalifies and re-arms; multiple markets
independent; repeated identical updates deterministic (armed once); expiry
boundary exact. Full suite: **211 passed, 0 failed, 2 ignored**.

## 15. Property / invariant results

Proved: unqualified never ARMED; unknown/stale/invalidated cannot arm; an armed
context cannot outlive invalidation/expiry; contexts are version-tied; consumed
cannot arm; markets are isolated; identical inputs give identical decisions.
M5 performs no network I/O, signing/submitting, capital arbitration, or position
creation — the arm path is a pure function of local shard state.

## 16. Live validation

`neurone run` (default Pyth mode), 55 s:

```
markets_evaluated        144,659
markets_qualified             42
markets_armed                 42   (every newly qualified market armed)
arm_refreshed              1,274   (refreshed in place, no re-count)
arm_invalidated                0
arm_expired                    0
sol_usd_reference_updates      1   (Pyth heartbeat)
strategy_eval p50            100 ns
```

An 85 s churn-heavy run showed `armed=66`, `refreshed=4,485`,
`arm_invalidated=4`, `arm_expired=7`. No trades; nothing signed or sent.

## 17. Performance

Combined M4+M5 evaluation p50 **100 ns**, p99 **1 µs** — identical to the
M4-only baseline (M5 adds a few compares and one `Option` write). Allocation-
free, shard-local, no task/lock/I-O/scan.

## 18. Files changed

```
src/market.rs      MarketStatus::Armed; ArmedContext (+ is_expired); armed field;
                   equality; size guard 768 -> 1024 (documented)
src/shard.rs       M5 arm/refresh + disarm on rejection; current_slot falls back to
                   the market's own last slot without global slot events;
                   qualification metric counts new episodes
src/strategy.rs    max_arm_age_slots (default 150, documented)
src/telemetry.rs   M5 counters + report line
config/default.toml  max_arm_age_slots = 150
tests/arming.rs    (new) 15 tests
tests/strategy.rs  status expectations Qualified -> Armed
docs/M5_PREARMING_REPORT.md
```

No change to M4 thresholds, the Pyth reference path, quote math, or the
ingest/route/shard architecture.

## 19. Remaining limitations

1. Idle markets keep an armed context until their next event (benign: no event =>
   no trigger), and M6 must re-check `is_expired` + `reserve_state()`.
   Deliberately no per-slot scan over armed markets.
2. The trigger condition is undefined, so M5 only provides the interface.
3. `max_arm_age_slots = 150` is a documented, configurable choice tied to
   `reserve_stale_slots`.
4. `arm_refreshed` is naturally high for continuously trading markets.

## 20. Confirmations

No transaction was signed; none was submitted; Beam was not used; no capital was
spent; no position was opened; no TP/SL, exit, or P&L was implemented; no
M6/M7 functionality was implemented. M5 remains shard-local, deterministic,
allocation-free, lock-free, and free of network/DB/filesystem I/O.
