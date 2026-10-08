# Neurone — Late-Observed Market State Semantics

Scope: verify and, only if necessary, correct the runtime so a market discovered
*after* launch is treated as a normal join-in-progress rather than as unusable.
No M4 work, no RPC/polling, no ingestion/shard redesign, no quote/execution
changes, no `previous_event_not_contiguous` root-cause work.

---

## 1. Current runtime behavior (found in code)

**Market creation / discovery.** A market is created the moment any *stateful*
event for its key is applied, in `src/shard.rs`:

* `EventKind::Account` — the shard looks the market up by `a.pubkey` and, if
  absent, `entry().or_insert_with(MarketState::new)`. For a pump.fun
  `BondingCurve` account the payload is decoded (`src/decode/pumpfun.rs`) and
  `apply_account` / `apply_decoded_account` sets `base_reserve`, `quote_reserve`,
  `virtual_*` and `reserves_known = true`.
* `EventKind::Transaction` — `market_mut` creates a market for decoded
  `creates` and `swaps`; `apply_swap` sets `reserves_known = true` from the
  event's reserves. A transaction that only *touches* a market
  (`apply_touch`) does **not** create it.

`src/engine.rs` routes by `hash(key) % num_shards` and fans a whole event out to
the owning shard(s); there is no eligibility filter, no "seen at launch" check,
and no per-market history requirement.

**Reserve trust model.** `MarketState::reserve_state(current_slot, stale_slots)`
(`src/market.rs`):

```
invalidated_at_slot.is_some()            -> Invalidated
else !reserves_known                     -> Unknown
else current_slot - last_reserve_slot > N -> Stale
else                                     -> Known
```

`reserves_known` becomes true only from an **authoritative** source: a decoded
bonding-curve account update or a decoded swap event carrying reserves. No code
path fabricates reserves, and `Known` is never assumed.

**Consequence for a late-observed market.** Nothing in the runtime requires the
market's launch, its earlier trades, or a contiguous predecessor chain. As soon
as the next authoritative Yellowstone update arrives (the default config
subscribes pump.fun bonding-curve accounts — `config/default.toml`,
`src/config.rs`), `apply_account` sets `reserves_known` and the market is
`Known`. A market first seen via a decoded swap is `Known` immediately. A market
seen only via a non-state transaction is not created at all (no phantom state).

**Is `previous_event_not_contiguous` coupled into the runtime?** No. A
repository-wide search shows `previous_event_not_contiguous` and
`QuoteError::UnsupportedState` occur **only in `src/validate.rs`** (the offline
M3 parity harness). The runtime (`engine`, `shard`, `market`, `quote` callers)
never reads either. They are a property of the validator's methodology, not a
runtime gate.

---

## 2. Exact semantic problem (if any)

**None found in the runtime.** The task's described failure mode — a
late-observed market being permanently `Unknown`/excluded, or
`previous_event_not_contiguous` rejecting a live market — does not exist in the
current production code. The runtime already implements:

* late discovery → bootstrap from the next authoritative Yellowstone state →
  `Known` → continuous observation, and
* fail-closed behavior for genuine uncertainty (`Stale` by slot age,
  `Invalidated` by a genuine non-trade reserve mutation),
* with no launch/history/predecessor requirement.

The only place the described semantics *appear* is the M3 parity validator, where
they are intentional and correct for their purpose (proving a specific
historical transition).

---

## 3. Exact changes made

No runtime behavior changed. The change set is a verification test suite plus
two documentation clarifications that make the runtime/validation distinction
explicit (per the task's instruction to document that
`previous_event_not_contiguous` is not a runtime eligibility gate):

1. **`tests/late_market.rs` (new, 5 deterministic tests)** — see §5. No
   production code was modified to make them pass; they pin the existing
   semantics.
2. **`src/market.rs`** — expanded the `ReserveState` doc: `Unknown` is the
   **normal transient bootstrap** of a market discovered after launch (clears on
   the next authoritative update; not a rejection), while `Stale`/`Invalidated`
   are the untrustworthy cases. Also corrected the `Invalidated` doc, which
   still named a fee sweep as the trigger (sweeps are proven not to be reserve
   mutations — see `docs/M3_3_SWEEP_RESERVE_DECODING_REPORT.md`).
3. **`src/validate.rs`** — module-doc note that `previous_event_not_contiguous`
   / `UnsupportedState` are **validation-only** classifications and not a
   runtime market-eligibility gate.

No change to `src/engine.rs`, `src/shard.rs` logic, `src/quote.rs` math,
`src/decode/*`, config/subscriptions, or the validator's classification logic.

---

## 4. Runtime vs validation semantics (explicit)

| Concern | Who | Question answered | Effect of a "gap" |
|---|---|---|---|
| Runtime scanner | `engine`/`shard`/`market` | "Can we establish and continuously maintain a trustworthy **current** state from Yellowstone?" | Keep `Unknown` until the next authoritative update; `Stale`/`Invalidated` fail closed. Never excludes on missing history. |
| Parity validator | `src/validate.rs` | "Can we prove this **historical transition** exactly from the observed predecessor?" | Count as `unsupported` (`previous_event_not_contiguous` etc.); does not touch runtime state. |

The validator's `UnsupportedState` is a statement about the validator's
evidence, not about the market's usability. The two must not be conflated; this
report records that boundary, and the code already respects it.

---

## 5. Tests

New: `tests/late_market.rs` (5 deterministic tests, real engine/shards):

1. `late_market_bootstraps_from_authoritative_account_update` — a market first
   seen via a bonding-curve account update at a slot long after launch becomes
   `Known`, with `last_reserve_slot` anchored to the account's own slot, and is
   quotable.
2. `late_market_requires_no_launch_or_predecessor_chain` — a market first seen
   via a mid-life swap becomes `Known`; a later swap whose reserves are wildly
   discontinuous (the exact shape the validator calls
   `previous_event_not_contiguous`) is applied continuously and stays `Known`.
3. `insufficient_current_state_is_not_falsely_known` — a `CreateEvent` seeds
   identity but not trust (`reserves_known == false`, state `Unknown`,
   `QuoteError::ReservesUnknown`); a genuine account update then makes it
   `Known`. A transaction that only *touches* a never-seen market creates no
   market (no fabrication).
4. `post_bootstrap_gap_still_fails_closed` — after bootstrap, slot-age freshness
   ages to `Stale`; a genuine non-trade reserve mutation yields `Invalidated`
   (quote refused), bumps `reserve_invalidations`, and a fresh authoritative
   account update recovers it to `Known` and bumps `reserve_revalidations`.
5. `discontinuous_successor_does_not_invalidate_runtime_market` — a
   non-chained successor swap leaves the market `Known` with
   `reserve_invalidations == 0`.

Existing invalidation/recovery tests remain unchanged and passing
(`tests/mutation.rs`, `tests/reserve_state.rs`, `tests/failed_transaction.rs`),
and the M3 parity tests are untouched.

---

## 6. Validation

| Command | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --all-targets --all-features -- -D warnings` | clean (0 warnings) |
| `cargo test --all-targets` | **113 passed, 0 failed, 1 ignored** |
| `cargo build --release --locked` | ok |
| `cargo test --test live_solami --locked -- --ignored` | **1 passed** |
| `NEURONE_VALIDATE_SECONDS=20 ./target/release/neurone validate` | parity unchanged: 5,615 / 5,615 supported exact, **0 mismatch, 0 errors**; `previous_event_not_contiguous=106` still reported by the **validator** (validation-only) |

`cargo test --all-targets` breakdown: lib 49 · architecture 8 · decode_and_state
6 · failed_transaction 4 · ingest_reconnect 1 · **late_market 5 (new)** ·
live_solami 0 (1 ignored) · m3_parity 12 · mutation 10 · quote_parity 5 ·
real_fixtures 7 · reserve_state 6 ⇒ 113 passed / 0 failed / 1 ignored.

Diff review: production changes are documentation only
(`src/market.rs`, `src/validate.rs` doc comments); the only additive code is the
new test suite. No runtime logic changed.

---

## 7. Remaining limitations

1. **pump.swap pools are not account-subscribed by default.** The default filter
   subscribes pump.fun bonding curves only, and `decode_account` for a pump.swap
   `Pool` returns `base_reserve = None` / `quote_reserve = None` (current
   reserves are not stored in the pool account). A late-observed pump.swap pool
   therefore stays `Unknown` until its next decoded swap establishes reserves.
   This is the intended fail-closed "insufficient current state" behavior, not a
   defect; adding pool bootstrap would be a separate, explicitly-scoped change.
2. **Markets seen only via a non-state transaction are not tracked** (no phantom
   market). They enter normal observation on their next stateful event. This is
   deliberate (no fabrication).
3. **`previous_event_not_contiguous` root cause** in the validator is left
   untouched (out of scope) and continues to be reported there.
4. `ReserveState::Unknown` intentionally remains non-quotable; the late-market
   improvement is that it is transient and self-clearing, not that it is
   quotable while unproven.

---

## 8. Verdict

`NO RUNTIME CHANGE REQUIRED — LATE-OBSERVED MARKETS ALREADY SUPPORTED`

The runtime already bootstraps a late-discovered market from the next
authoritative Yellowstone update and then tracks it continuously, with no
launch, history, or predecessor-continuity requirement, while preserving
fail-closed `Stale`/`Invalidated` behavior for genuine current-state
uncertainty. `previous_event_not_contiguous` is confirmed validation-only and is
not a runtime eligibility gate. The change set adds deterministic tests pinning
these semantics and documents the runtime/validation boundary; no production
behavior changed.
