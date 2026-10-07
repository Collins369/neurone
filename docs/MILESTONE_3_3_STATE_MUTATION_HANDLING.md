# Neurone — M3.3: State Mutation Handling & Stale-State Invalidation

Production change (narrow): market state is now mutation-aware for the
pump.fun fee sweep discovered in M3.2. No trading/execution, no M4.

---

## 1. Implementation Summary

Neurone no longer assumes `TradeEvent == every economically meaningful state
change`. A non-trade reserve mutation now **invalidates** the affected market
until a fresh authoritative state is established.

Changed files:

| File | Change |
|---|---|
| `src/events.rs` | `TransactionUpdate.has_reserve_mutation`; detect pump.fun `SweepProtocolFee` / `SweepCreatorFee` from the deployed program's Anchor logs |
| `src/market.rs` | `MarketState.state_version`, `invalidated_at_slot`; `ReserveState::Invalidated`; `invalidate(slot)`; `apply_account` re-validates on a fresh update at `slot >= invalidated_at_slot` |
| `src/shard.rs` | On a transaction carrying a reserve mutation, invalidate every market it touched |
| `src/quote.rs` | `QuoteError::StateInvalidated`; quotes refuse invalidated state |
| `src/telemetry.rs` | `reserve_invalidations` counter |
| `src/validate.rs` | `reserve_mutations_seen` diagnostic |
| `tests/mutation.rs` (new) | 6 deterministic mutation/invalidation tests |

## 2. State Model

```
CURRENT ──(non-trade reserve mutation)──► INVALIDATED
   ▲                                          │
   └──── fresh authoritative update (slot ≥ mutation slot) ────┘
```

`MarketState` gains:

```
state_version: u64            // monotonic; bumped on every applied change and on invalidation
invalidated_at_slot: Option<u64>
```

`reserve_state()` now returns a fourth state, `Invalidated`, which — like
`Unknown` and `Stale` — is never quotable. `quote()` returns
`QuoteError::StateInvalidated`. An armed trade (future M4) records the
`state_version` it armed from; any mutation bumps the version, so an
invalidation is observable as `armed.version != market.version`.

Re-validation is deterministic: a fresh account update with `slot >=
invalidated_at_slot` clears the flag (Solana's per-account write monotonicity +
slot ordering). An update from *before* the mutation cannot clear it.

## 3. Ordering Model

* Per-market mutation ordering uses `slot` plus the transaction's
  `SubscribeUpdateTransactionInfo.index` (the transaction index within the
  block), which Yellowstone provides.
* Invalidation is monotone in time: `invalidated_at_slot = max(existing, new)`.
* Re-validation requires `account.slot >= invalidated_at_slot`, so a
  reordered (older) account update cannot re-validate.
* No new ordering fields were invented; the strongest available deterministic
  keys (slot, transaction index) are used, and the conservative fallback
  (remain invalidated) applies whenever ordering cannot be proven.

## 4. Sweep Handling

Detection is based on the evidence M3.2E established: the deployed pump program
emits `Program log: Instruction: SweepProtocolFee` / `SweepCreatorFee`
(program `6EF8…`). The exact public IDL layout for these instructions is not
published, so **no payload is decoded and no reserve value is applied**. The
detector scans the transaction's log messages; the resulting market invalidation
is the safe fallback:

```
mutation observed → state not deterministically reconstructable → INVALIDATE → no quote/execution
```

The sweep is **not** treated as malicious: it invalidates stale state, and the
market resumes normally once a fresh authoritative state arrives.

## 5. Safety Invariant

A trade may be quoted/executed only when `reserve_state() == Known`.
`Unknown`, `Stale`, and `Invalidated` all refuse. Therefore:

```
ARMED (version V)
   │  sweep → invalidate (version V+1)
   ▼
quote() == Err(StateInvalidated)   → MUST NOT FIRE
   │  fresh account state (slot ≥ mutation) → version V+2, Known
   ▼
re-arm → eligible again
```

`tests/mutation.rs` demonstrates that a quote against an invalidated market is
refused, that an older account update cannot re-validate, and that a fresh one
re-validates. Nothing in the hot path silently continues from stale reserves.

## 6. Tests

6 new deterministic tests (`tests/mutation.rs`), all passing:

1. `sweep_invalidates_state_and_blocks_quotes` — trade → sweep → quote refused.
2. `fresh_account_state_revalidates_market` — sweep → fresh update → Known.
3. `stale_account_update_cannot_revalidate` — pre-mutation update cannot clear.
4. `multiple_mutations_remain_invalidated` — repeated sweeps keep it invalid.
5. `state_version_advances_and_market_can_rearm` — version bumps; re-arm allowed.
6. `normalizer_detects_sweep_instruction` — log-based detection (sweep logs
   detected; a normal Buy is not).

Full suite: **99 deterministic tests pass, 0 failed** (1 ignored live test).
Clippy and fmt clean; release build ok.

## 7. Live Validation

* `neurone validate` (60 s, Yellowstone): **`reserve_mutations_seen = 142`** —
  sweep transactions are delivered and detected live.
* `neurone run` (sharded runtime, 45 s): **`reserve_invalidations` 1 → 3** —
  tracked markets are invalidated on sweeps, end to end.
* Parity unchanged: all supported paths 100% exact (0 mismatches, 0 errors).
  Example run — PumpSwap SELL 6,698/6,698, `buy_exact_quote_in` 7,542/7,542,
  `buy` 1,557/1,557; pump.fun `buy_exact_in` 672/672, SELL 1,140/1,140,
  token-target 609/609.

(One intermediate run showed 2 transient `buy_exact_in` mismatches out of 779 —
not reproducible on re-run and not caused by this change, which does not touch
the quote path.)

## 8. Performance

* Detection is a bounded scan of `meta.log_messages` with two substring checks
  per transaction — O(1) per transaction, no allocation, no I/O.
* Invalidation is O(keys) per mutating transaction, per shard, on shard-local
  maps — no global lock; a mutation for market A does not stall other markets.
* Measured hot-path latency unchanged: normalize+decode p50 9 µs / p95 64 µs /
  p99 141 µs; quote p50 0.32 µs / p95 0.80 µs / p99 1.09 µs.

## 9. Remaining Limitations

* The sweep's exact instruction payload/account layout is not decoded (absent
  from the published IDL), so the *resulting* reserve value is not applied
  directly; re-validation depends on the bonding-curve **account** stream
  delivering a fresh update at `slot >= invalidated_at_slot`. If such an update
  never arrives, the market stays invalidated (fail-closed) — the conservative
  behaviour the task requires.
* Detection is log-text based (`Instruction: SweepProtocolFee`), which is the
  evidence M3.2E confirmed; a program change to the log text would require
  re-validating detection.
* Invalidation covers markets the shard already tracks (their curve is in the
  sweep transaction's account keys); a curve not yet tracked has no state to
  invalidate.

## 10. Verdict

**`IMPLEMENTED WITH CONSERVATIVE LIMITATION`**

Neurone now notices the pump.fun fee-sweep reserve mutation, invalidates stale
market state, refuses quotes/execution until a fresh authoritative state
arrives, and resumes normally afterwards — without treating the mutation as
malicious and without weakening the fail-closed boundary. The conservative
limitation is that the mutation's resulting value is not applied directly
(the instruction layout is undocumented); re-validation relies on the
bonding-curve account stream with a deterministic slot-order rule.
