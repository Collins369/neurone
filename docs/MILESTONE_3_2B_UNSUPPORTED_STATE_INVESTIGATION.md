# Neurone — M3.2B: Pump.fun Unsupported-State Investigation

Focused investigation + minimal fix. No trading/execution. No formula change.

Statuses: `CONFIRMED`, `OBSERVED`, `INFERRED`, `HYPOTHESIS`, `UNRESOLVED`.

---

## 1. Number of Exclusions Examined

M3.2A excluded **1,654** pump.fun samples: 893 SELL + 761 token-target BUY.
Reason instrumentation was added to the gate and a 60 s live run captured the
actual rejection reasons for every excluded trade.

## 2. Pump.fun SELL Exclusions Examined

893 (M3.2A) / 329 (after fix). Reason histogram showed the same two causes as
token-target (below); no SELL-specific cause.

## 3. Token-Target BUY Exclusions Examined

761 (M3.2A) / 330 (after fix). Same two causes.

## 4. Exact Reason(s) They Failed

Two concrete, reproducible causes were found — both **correlation bugs in the
validation gate**, not protocol or formula issues:

1. **`previous_event_not_contiguous` (dominant).** The gate stored the previous
   observed event's **pre-state** instead of its **post-state**, so the
   comparison against the next trade's pre-state failed for almost every pair.
2. **Same-slot trades rejected.** Contiguity required `prev.slot < trade.slot`
   strictly, so two trades on the same curve in the same slot (common for hot
   curves) could never corroborate.

`no_previous_event_observed` (the first trade seen on a curve inside the run
window) is a third, legitimate cause: no predecessor event exists, and the
account cache had no matching older state for those trades.

## 5. Evidence

Instrumented reason counts for the M3.2A-style run (60 s):

```
no_previous_event_observed    = 262
previous_event_not_contiguous = 1194
```

The second bucket dominated. Inspecting the gate showed the stored value was
`swap_pre_state(prev).pre_*` (the previous trade's *pre*) rather than the
event's own stored virtual reserves (its *post*). A trade's true pre-state is
the previous trade's post-state, so the comparison was systematically off by
one trade. The strict `slot <` additionally blocked same-slot successions.

## 6. Minimal Fix Implemented

In `src/validate.rs` (validation gate only — production quote math untouched):

* store the previous event's **post**-trade state
  (`swap.virtual_base_reserve`, `swap.virtual_quote_reserve`) as the
  contiguity anchor; and
* accept a same-slot predecessor (`prev.slot <= trade.slot`) — the post-state
  **equality** remains the actual corroboration, so this does not weaken safety.

No formula, fee, decoder or architecture change; `UnsupportedState` is still
returned whenever the pre-state equality is not satisfied.

## 7. Before / After Parity Results

Live Solami Yellowstone, release, 60 s (M3.2A run → M3.2B run):

| Protocol / Instruction | Samples | Exact | Mismatch | Errors | Unsupported | Parity |
|---|---:|---:|---:|---:|---:|---:|
| PumpSwap SELL | 11,866 | 11,866 | 0 | 0 | 0 | 100% |
| PumpSwap `buy_exact_quote_in` | 7,714 | 7,714 | 0 | 0 | 0 | 100% |
| PumpSwap `buy` | 1,431 | 1,431 | 0 | 0 | 0 | 100% |
| Pump.fun `buy_exact_*` | 689 | 689 | 0 | 0 | 0 | 100% |
| Pump.fun SELL | 1,400 (was 747) | 1,400 | 0 | 0 | 329 (was 893) | 100% of supported |
| Pump.fun token-target `buy` | 876 (was 445) | 876 | 0 | 0 | 330 (was 761) | 100% of supported |

```
UnsupportedState before  = 1,654
UnsupportedState after   = 659
UnsupportedState reduced = 995  (-60%)
newly-supported samples  = 995
remaining mismatches     = 0
regressions              = 0
```

`acct_corroborated` rose from 1,556/3,603 (43%) to 2,920/3,624 (81%).

## 8. Number of Exclusions Removed

**995 of 1,654 (60%)**, across both instruction types (SELL −564,
token-target −431).

## 9. Remaining Exclusions and Their Known Reason

659 remain, with recorded reasons:

* `no_previous_event_observed` = 202 — the first trade observed on a curve in
  the run window, with no matching older account state cached (the account
  update had not arrived yet or the curve had no prior cached state).
* `previous_event_not_contiguous` = 457 — the previous observed event is not
  the immediate predecessor (a trade was not observed between them, or the
  curve was mutated outside a `TradeEvent`). These are the M3.2
  "intermediate mutation / missed event" candidates; they stay `UnsupportedState`
  until transaction-local evidence corroborates the pre-state.

## 10. Regressions

None. PumpSwap (all three), pump.fun exact-in buys, and already-supported
pump.fun SELL / token-target remain 100% exact; 93 deterministic tests pass;
clippy/fmt clean. No production quote/decoder change; the `UnsupportedState`
guarantee is intact (equality is still required).

## 11. Final Verdict

**`PARTIALLY RESOLVED`**

* The dominant exclusion cause was a **validation-gate correlation bug**
  (previous event's pre-state stored instead of its post-state; same-slot
  successions rejected). Fixed minimally; exclusions fell 60% (1,654 → 659)
  with **zero newly-introduced mismatches** and no regression.
* The remaining 659 exclusions have two documented, non-formula causes
  (202 no-predecessor, 457 non-contiguous) and remain safely excluded.

Stopping here per scope. Not proceeding to M4.
