# Neurone — Post-Audit Required Safety Fixes

Scope: the two substantiated defects from
`docs/M1_M2_M3_CODEBASE_AUDIT.md`, and nothing else.

* **Fix A** — failed (reverted) transactions must not mutate market state.
* **Fix B** — M3.3 invalidation must be recoverable under the shipped/default
  runtime configuration.

No M4 work (no arming, trigger, strategy, capital, execution, Beam, TP/SL), no
protocol/quote math changes, no architecture changes.

---

## A. Fixes Implemented

### Fix A — Failed transactions must not mutate market state

**Problem.** `TransactionUpdate.success` was computed in `src/events.rs` but
never consumed. `src/shard.rs` applied a transaction's decoded `swaps`,
`creates`, and reserve-mutation invalidation regardless of `success`. A reverted
transaction whose program logs still contained an Anchor `Program data:` event
could therefore move `MarketState` reserves, create a market, or invalidate one.

**Implementation.** A single centralized guard in the transaction-application
path (`src/shard.rs`, `EventKind::Transaction` arm): the economic mutations —
reserve-mutation invalidation, `apply_create`, and `apply_swap` — are applied
only when `t.success` is true.

```rust
if t.success {
    if t.has_reserve_mutation { /* invalidate touched markets */ }
    for created in &t.creates { /* apply_create */ }
    for swap in &t.swaps { /* apply_swap */ }
}
```

The pure-observability touch (`apply_touch`) still runs for failed transactions,
so `tx_count`/`last_tx_slot` and telemetry keep reflecting that the market was
involved; only *authoritative state* is protected. No decode, normalization, or
protocol change was made.

**Files changed:** `src/shard.rs`; new `tests/failed_transaction.rs`.

**Why it is safe.** Failed transactions are rolled back on chain, so ignoring
their decoded contents cannot drop a state change that actually happened.
Successful transactions take exactly the previous code path (same operations,
same order), so behavior is unchanged for them.

**Why it is minimal.** One condition wrapping the existing block; no new
types, fields, or decode changes.

### Fix B — Recoverable M3.3 invalidation under the default runtime

**Problem.** `MarketState::invalidate()` marks a market `Invalidated`, and
`apply_account()` clears the flag only for a fresh authoritative account update
with `slot >= invalidated_at_slot`. `apply_swap()` does not clear it. The
shipped `config/default.toml` set `account_programs = []` / `account_addresses =
[]`, so the default `neurone run` path received **no** bonding-curve account
updates — a swept market could never return to `Known`.

**Implementation.** The shipped/default configuration now subscribes the
pump.fun bonding-curve accounts:

* `src/config.rs::FilterConfig::default()` sets
  `account_programs = [pump.fun]` and
  `account_memcmp_base58 = base58(BONDING_CURVE_DISC)` (computed from
  `decode::pumpfun::BONDING_CURVE_DISC`, so the discriminator has one source of
  truth).
* `config/default.toml` mirrors this (`account_programs = ["6EF8…"]`,
  `account_memcmp_base58 = "4y6pru6YvC7"`).

The recovery mechanism itself is the **existing** `apply_account` rule (already
unit/integration tested) — it was not rewritten. A new telemetry counter
`reserve_revalidations` (incremented in `src/shard.rs` when an account update
clears an invalidation) makes the recovery observable end to end.

**Files changed:** `src/config.rs`, `config/default.toml`, `src/shard.rs`
(revalidation counter), `src/telemetry.rs` (counter + report line),
`src/ingest/solami.rs` (test updates only), `tests/mutation.rs`.

**Why it is safe.** The authoritative source is the bonding-curve **account** —
the exact on-chain state, not a reconstruction. The recovery rule is unchanged
and conservative: a market becomes quotable again only after a fresh account
update at `slot >= invalidated_at_slot`, and the counter is only bumped on that
transition. `apply_swap` deliberately does **not** clear invalidation (see §B),
so the fix does not relax the fail-closed boundary.

**Why it is minimal.** It enables an account subscription the codebase already
supported (it is the same memcmp-narrowed filter the M3 validation harness
uses), rather than introducing a new recovery heuristic; no new module, no new
service, no polling/RPC.

---

## B. Recovery Semantics

Final, implemented transition (no hypotheticals):

```text
KNOWN
  │  non-trade reserve mutation observed in a *successful* transaction
  ▼
INVALIDATED            (invalidated_at_slot = slot; reserve_state() -> Invalidated; quote() -> StateInvalidated)
  │  fresh authoritative ACCOUNT update for this market with slot >= invalidated_at_slot
  ▼
KNOWN                  (invalidated_at_slot cleared; reserve_state() then applies the normal freshness rule)
```

Explicit, enforced non-transitions (fail-closed):

* `INVALIDATED → a swap/TradeEvent → KNOWN` is **not** allowed. A trade updates
  reserves via `apply_swap` but does not clear `invalidated_at_slot`;
  `reserve_state()` still returns `Invalidated`. This is tested
  (`swap_alone_does_not_recover_invalidated_state`).
* `INVALIDATED → an older account update (slot < invalidated_at_slot) → KNOWN`
  is **not** allowed (tested by `stale_account_update_cannot_revalidate`).

Once recovered, the market is usable only when the **existing** freshness rule
permits (`reserve_state() == Known` within `reserve_stale_slots`); recovery does
not bypass staleness.

---

## C. Tests

### Focused tests added / changed

`tests/failed_transaction.rs` (new, 4 tests):

1. `successful_transaction_applies_swap` — a successful tx still applies its swap.
2. `failed_transaction_does_not_apply_swap` — reserves/trade state unchanged.
3. `failed_transaction_does_not_create_market` — failed create makes no market;
   successful create does.
4. `failed_reserve_mutation_does_not_invalidate` — failed sweep leaves `Known`;
   successful sweep invalidates.

`tests/mutation.rs` (2 tests added, rig extended with metrics):

5. `recovery_is_observable_via_metrics` — sweep bumps `reserve_invalidations`; a
   fresh authoritative account update bumps `reserve_revalidations` and returns
   the market to `Known`.
6. `swap_alone_does_not_recover_invalidated_state` — a later trade does not
   clear invalidation (`reserve_revalidations` stays 0).

`src/ingest/solami.rs` (unit tests):

7. `default_subscription_targets_bonding_curves` (new) — the default
   configuration's subscribe request contains an account filter with
   `owner = pump.fun` and the `4y6pru6YvC7` (BondingCurve) memcmp at offset 0;
   this is the runtime/default-config proof that the shipped path now receives
   the authoritative recovery updates.
8. `request_includes_scoped_filters` (updated) — reflects that the default now
   subscribes the bonding-curve accounts.

### Full validation (run in this task)

| Command | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --all-targets --all-features -- -D warnings` | clean (0 warnings) |
| `cargo build --release --locked` | ok |
| `cargo test --all-targets --locked` | **106 passed, 0 failed, 1 ignored** |

`cargo test --all-targets --locked` breakdown:

```
lib unit tests 49 · architecture 8 · decode_and_state 6 · failed_transaction 4
ingest_reconnect 1 · live_solami 0 (1 ignored) · m3_parity 12 · mutation 8
quote_parity 5 · real_fixtures 7 · reserve_state 6   => 106 passed / 0 failed / 1 ignored
```

### Live validation (run in this task, read-only network)

* `cargo test --test live_solami -- --ignored --nocapture` → **passed**
  (authenticated stream, live decode, live quote).
* `neuron validate` (25 s, release) → parity unchanged: **10,310 / 10,310
  supported samples exact, 0 mismatch, 0 errors**; unsupported 410, all
  classified (`no_previous_event_observed=126`,
  `previous_event_not_contiguous=284`); `reserve_mutations_seen=10`.
* `neurone run` (60 s, release, **default configuration**) → the shipped path
  now observes the full invalidate→recover lifecycle end to end:
  `reserve_invalidations` reached **15** and `reserve_revalidations` reached
  **1** (i.e., a swept market was returned to `Known` from a fresh authoritative
  account update). Under the pre-fix configuration this counter could never
  move.

---

## D. Files Changed

```
config/default.toml                 (subscribe pump.fun bonding-curve accounts)
src/config.rs                       (FilterConfig::default: account program + memcmp disc)
src/shard.rs                        (Fix A guard; revalidation telemetry)
src/telemetry.rs                    (reserve_revalidations counter + report line)
src/ingest/solami.rs                (tests only: default-subscription coverage)
tests/failed_transaction.rs         (new: Fix A regression tests)
tests/mutation.rs                   (Fix B regression tests + recovery metric)
docs/M1_M2_M3_REQUIRED_FIXES_REPORT.md  (this report)
```

No other file was modified. `src/market.rs`, `src/quote.rs`, `src/decode/*`,
`src/engine.rs`, `src/runtime.rs`, and `src/validate.rs` are untouched.

---

## E. Files Intentionally NOT Changed

* **M4 pre-state corroboration gate** — still only in `src/validate.rs`
  (`UnsupportedState`), not moved or rewritten.
* **Quote-engine production wiring** — `quote()` remains off the runtime path;
  no quote→strategy→arm wiring was added.
* **ARM / trigger / strategy / capital / execution / Beam / entry / exit /
  TP-SL** — none implemented.
* **Transaction-index ordering** — `TransactionUpdate.index` remains unused; the
  recovery fix did not require it (recovery is by account slot, which the
  existing rule already used).
* **Protocol math** — pump.fun/pump.swap formulas, fee primitives, decoder
  layouts, and quote math unchanged.
* **Architecture** — no global locks, no DB/RPC/HTTP in the hot path, no new
  service, no polling, shard ownership unchanged.

---

## F. Remaining Limitations

1. **Recovery is delayed to the next authoritative account write.** Because
   account and transaction updates are delivered on separate subscriptions,
   a swept curve's own account update is often processed *before* the sweep
   transaction invalidates it (same slot). The market is therefore restored only
   when the curve is next written on chain (its next trade/state change). A
   market that is never written again stays `Invalidated` — the intended
   fail-closed behavior, but recovery is not instantaneous. This is why live
   60 s runs show far more invalidations than revalidations (15 vs 1).
2. **The sweep payload is still not decoded** (absent from the published IDL);
   the resulting reserve value is not applied directly (unchanged from M3.3).
3. **The default subscription is slightly larger.** It now streams all pump.fun
   `BondingCurve` accounts (memcmp-narrowed). Observed live at a modest rate
   (~1,600–2,200 curve updates / 25 s, ~160–200 curves cached, no startup
   flood, `curve_startup=0`); `neurone run` tracked ~6,300 markets in 60 s.
   `config/live_probe.toml` still selects the narrow slots-only probe.
4. **Out-of-scope M4 items remain** by design: the pre-state corroboration gate
   is not in the runtime, and `quote()` has no production consumer yet.

---

## G. Final Verdict

### `FIXES COMPLETE — READY FOR M4 WITH LIMITATIONS`

Both substantiated defects are fixed, deterministically tested, and confirmed
on live data:

* Failed transactions no longer mutate market state (Fix A) — 4 new regression
  tests; successful behavior unchanged.
* M3.3 invalidation is now recoverable under the shipped/default configuration
  (Fix B) — the default runtime subscribes the authoritative bonding-curve
  accounts; recovery lifecycle observed live (`invalidations 15`,
  `revalidations 1`). The limitation is that recovery is conservative and waits
  for the next authoritative account write (§F.1).

No M4 work was started. The remaining limitation is inherent to safe,
fail-closed recovery rather than an implementation gap.
