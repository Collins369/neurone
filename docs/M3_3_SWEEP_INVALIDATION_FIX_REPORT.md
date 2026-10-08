# Neurone — M3.3: Sweep Invalidation Fix

Small targeted M3 cleanup. No M4 work, no architecture changes, no quote math
changes, no M3.2 predecessor/parity changes, no sweep reserve decoder.
Authoritative basis: `docs/M3_3_SWEEP_RESERVE_DECODING_REPORT.md`.

---

## 1. Summary

Pump.fun fee sweeps (`SweepProtocolFee` / `SweepCreatorFee`) are fee transfers.
The M3.3 on-chain proof (30/30 real samples, both instruction types) shows
`virtual_quote_reserves` and `virtual_token_reserves` are byte-identical before
and after every sweep. The runtime nevertheless classified sweep log detection
as `TransactionUpdate.has_reserve_mutation = true`, which caused a conservative,
now-known-spurious `ReserveState::Invalidated`.

This change removes sweeps from the reserve-mutation set. Sweeps are still
**detected** and reported — as a new observability flag `has_sweep` — but they no
longer invalidate market state. The genuine reserve-mutation invalidation path is
untouched and still tested.

---

## 2. Exact code changes

### `src/events.rs`

* Split the single classification into two independent signals on
  `TransactionUpdate`:
  * `has_reserve_mutation` — a non-trade instruction that **genuinely** mutates
    the price-producing reserves (feeds the shard's invalidation path).
  * `has_sweep` (new) — a pump.fun fee sweep was present (**observability only**;
    must never invalidate).
* `detect_reserve_mutation` no longer matches `SweepProtocolFee` /
  `SweepCreatorFee`. With no other instruction currently known to mutate reserves
  without a `TradeEvent`, it returns `false`. It is kept (with a doc comment) so
  the "set of events" and the invalidation path remain wired for a future genuine
  mutation.
* Added `detect_sweep` (same bounded log scan as before) feeding `has_sweep`.
* `normalize_transaction` now sets `has_reserve_mutation = detect_reserve_mutation(info)`
  (currently always false) and `has_sweep = detect_sweep(info)`.

### `src/shard.rs`

* No behavioral change. The invalidation block still runs on
  `t.has_reserve_mutation` (so real reserve-mutating transactions still
  invalidate). The comment now states that the normalizer currently never sets
  the flag and why the fail-closed path is retained.

### `src/validate.rs`

* Diagnostic rename only: `ValidationReport::reserve_mutations_seen` →
  `sweeps_seen`, now incremented on `tx.has_sweep` (a sweep is not a reserve
  mutation). Output line `reserve_mutations_seen=` → `sweeps_seen=`. This keeps
  sweep observability in the live harness without mislabeling it.

### `src/ingest/simulated.rs`

* Set `has_sweep: false` on the synthetic transaction (no sweeps in the offline
  source).

### Tests (constructors + focused tests)

* `tests/{architecture,decode_and_state,failed_transaction,reserve_state}.rs`:
  added the new `has_sweep: false` field to their `TransactionUpdate` literals.
* `tests/mutation.rs`: see §4.

No change to `src/quote.rs`, `src/market.rs`, `src/decode/*`, the M3.2
predecessor/parity logic, or the parallel architecture.

---

## 3. Why sweeps are no longer reserve mutations

From the authoritative report (`docs/M3_3_SWEEP_RESERVE_DECODING_REPORT.md`):

* Both instructions carry only an 8-byte discriminator (no arguments) and emit a
  153-byte `SweepBondingCurveFeeEvent` whose `amount` is the **fee transferred**.
* In 30/30 real swept curves, the `BondingCurve` account's `virtual_quote_reserves`
  (offset 16) and `virtual_token_reserves` (offset 8) were identical before and
  after the sweep; the sweep instead debits the curve's lamports and zeroes its
  `creator_fee` / `protocol_fees` fields.
* Applying the swept `amount` to the reserves produces `pre − amount ≠ post` in
  every sample, i.e. it would be wrong.

Therefore a sweep is a fee movement, not a price-producing reserve mutation, and
must not invalidate market state. It is retained as an observability signal
(`has_sweep`) because the sweep is a real, useful protocol event (it changes the
curve's lamports and fee fields).

---

## 4. Tests added / updated

Added (deterministic, `tests/mutation.rs`):

* `protocol_fee_sweep_does_not_invalidate_known_market` — a `SweepProtocolFee`
  transaction built from the real Anchor log text, normalized through the real
  ingestion boundary, routed to a shard whose market is `Known`, leaves the
  market `Known` with `invalidated_at_slot == None`, is not blocked by
  `QuoteError::StateInvalidated`, and bumps `reserve_invalidations` by 0.
* `creator_fee_sweep_does_not_invalidate_known_market` — same for
  `SweepCreatorFee`.
* `normalizer_classifies_sweep_as_observability_not_mutation` — a protocol and a
  creator sweep both yield `(has_sweep, has_reserve_mutation) == (true, false)`.

Updated:

* `sweep_invalidates_state_and_blocks_quotes` → renamed
  `reserve_mutation_invalidates_state_and_blocks_quotes`; it uses a synthetic
  *genuine* reserve-mutation event (not a sweep) and still proves a genuine
  mutation invalidates and blocks quotes.
* `mutation_event` helper documented as a genuine (non-sweep) reserve mutation.

Unchanged and still passing (recovery / genuine invalidation behavior preserved):

* `fresh_account_state_revalidates_market`
* `stale_account_update_cannot_revalidate`
* `multiple_mutations_remain_invalidated`
* `swap_alone_does_not_recover_invalidated_state`
* `state_version_advances_and_market_can_rearm`
* `recovery_is_observable_via_metrics`
* `tests/failed_transaction.rs` (incl. `failed_reserve_mutation_does_not_invalidate`)

---

## 5. Validation

| Command | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --all-targets --all-features -- -D warnings` | clean (0 warnings) |
| `cargo test --all-targets` | **108 passed, 0 failed, 1 ignored** |
| `cargo build --release --locked` | ok |
| `cargo test --test live_solami --locked -- --ignored` | **1 passed** |
| `NEURONE_VALIDATE_SECONDS=20 ./target/release/neurone validate` | parity unchanged: 3,892 / 3,892 supported exact, **0 mismatch, 0 errors**; `sweeps_seen=0` in that window (label present); unsupported 115 all classified (`no_previous_event_observed=68`, `previous_event_not_contiguous=47`) |

`cargo test --all-targets` breakdown: lib 49 · architecture 8 · decode_and_state 6
· failed_transaction 4 · ingest_reconnect 1 · live_solami 0 (1 ignored) · m3_parity
12 · mutation **10** (was 8; +2) · quote_parity 5 · real_fixtures 7 · reserve_state
6 ⇒ 108 passed / 0 failed / 1 ignored.

Diff review: only `src/events.rs`, `src/shard.rs` (comment), `src/validate.rs`
(`sweeps_seen` rename + predicate), `src/ingest/simulated.rs` (new field), the
five test files, and this report changed. Quote math, decoders, `MarketState`,
the M3.2 parity/predecessor logic and the engine/shard architecture are
untouched.

---

## 6. Remaining limitations

1. `has_reserve_mutation` is now never set by the normalizer, because no
   non-trade instruction is currently proven to mutate the reserves. The flag,
   the shard invalidation block and its tests are retained so a future genuine
   mutation is a one-line wiring change.
2. Sweeps still `apply_touch` the markets they involve (transaction observation),
   so `tx_count` / `last_tx_slot` continue to update; only authoritative
   *invalidation* is removed. This is intended.
3. Sweep detection remains log-text based (`Instruction: SweepProtocolFee` /
   `SweepCreatorFee`), not discriminator based; a program log change would need
   re-validation. Out of scope here.
4. The unexplained `previous_event_not_contiguous` root cause is **not**
   addressed (explicitly out of scope) and remains observed live.

---

## 7. Verdict

`FIXED` — sweeps are no longer classified as reserve mutations, so they no longer
invalidate `Known` markets; genuine reserve-mutation invalidation and recovery
are preserved and tested; full suite (108 tests) and live parity pass.
