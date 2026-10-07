# Neurone — M3.2C: Remaining Unsupported-State Investigation

Investigation only. No production change shipped. The M3.2B gate is unchanged.

Statuses: `CONFIRMED`, `OBSERVED`, `INFERRED`, `HYPOTHESIS`, `UNRESOLVED`.

---

## 1. Executive Summary

The two remaining exclusion classes were investigated with **per-sample
instrumentation** (reason, mint, slot, previous-event slot, derived pre-state,
account-cache slots) rather than aggregate counters only.

Findings:

* `no_previous_event_observed` (~190–200) — `CONFIRMED`: these are the **first
  trade observed on a curve inside the run window**. The account cache holds
  only the state at the trade's own slot (`cache_slots == [trade_slot]`), i.e.
  the post-trade state, and `pre_state(trade_slot)` requires a strictly older
  slot → nothing available. No earlier event exists in the run; no earlier
  account state had been observed.
* `previous_event_not_contiguous` (~460–630) — `INFERRED`: the previous observed
  event on the curve is **not the immediate predecessor** (its stored
  post-state ≠ this trade's derived pre-state), with **no intermediate state**
  in the account cache between them. So the gap is a **missing predecessor
  event** (or a reserve mutation not emitted as a `TradeEvent`), not a stale
  cache entry.

**A concrete "delivery lag" fix was implemented and tested, then reverted
because it did not help** (see §7): a per-curve 32-deep event post-state history
plus deferred corroboration retried at end-of-run against the fully-populated
event/account history. Exclusion counts did **not** fall (they tracked traffic
only), which **disproves** the "account/event data simply arrives late"
hypothesis. Because no safe, evidence-backed Neurone fix could be proven, no
change was shipped and the fail-closed gate stands.

## 2. Before / After

| Metric | M3.2B | M3.2C |
|---|---:|---:|
| Supported/exact (all paths) | 100% | 100% |
| Mismatches | 0 | 0 |
| `no_previous_event_observed` | 202 | ~193 (traffic-dependent) |
| `previous_event_not_contiguous` | 457 | ~460–630 (traffic-dependent) |
| Net change from M3.2C | — | **none shipped** |

Absolute counts vary run-to-run with live traffic; the *rate* is stable
(~25–30% of pump.fun SELL/token-target trades).

## 3. `no_previous_event_observed` Analysis

Instrumented examples (all with `prev_slot = None`):

```
key=9TKkuJ9C… slot=454315523 pre=(572367018699427,2819971420857) prev_slot=None cache_slots=[454315523]
key=8m7uorAi… slot=454315523 pre=(976292620852845,18165145756)  prev_slot=None cache_slots=[454315523]
key=HXRu4UNZ… slot=454316341 pre=(1073000000000000,30000000000) prev_slot=None cache_slots=[454316341]
```

`cache_slots` contains only the trade's own slot (the post-trade update for that
same trade). Interpretation: **cause A — the first trade of that curve visible
in the run window.** No earlier `TradeEvent` exists in the run, and no earlier
account state had been observed, so no independent pre-state is available.
`CONFIRMED`. (Distinct curves, not one curve repeatedly.)

## 4. `previous_event_not_contiguous` Analysis

Instrumented examples:

```
key=58aR97Cv… slot=454315524 prev_slot=Some(454315523) cache_slots=[454315522, 454315524]
key=96ZZVxo6… slot=454315524 prev_slot=Some(454315522) cache_slots=[454315522, 454315524]
key=HQTDYDjY… slot=454315526 prev_slot=Some(454315524) cache_slots=[454315524, 454315526]
```

The predecessor event's stored post-state does not equal the trade's derived
pre-state, and there is no cached account state strictly between the two slots.
Checks performed:

* **Was a `TradeEvent` genuinely missed?** `INFERRED` yes — an intermediate
  trade on the same curve is the most parsimonious explanation given the
  contiguous state transition `pre = post_prev` fails by a whole-trade delta.
* **Was the intermediate transaction filtered out?** `HYPOTHESIS` — the
  subscription is scoped to pump.fun/pump_amm `account_include`; a trade routed
  through a program path whose log attribution was not resolved would be
  dropped. Program-attribution gaps were not observed directly, but they are the
  leading candidate for a genuinely missed event.
* **Did the curve mutate without a `TradeEvent`?** `HYPOTHESIS` — buyback /
  holder-reward / cashback instructions can move reserves; the decode currently
  extracts no such event.
* **Delivery lag?** `DISPROVEN` — see §7.
* **Validation assumption?** `DISPROVEN` for the M3.2B gate: it requires an
  exact state equality, and the state genuinely is not present.

## 5. Representative Evidence

| Observation | Value (example) | Meaning |
|---|---|---|
| trade slot | 454315524 | current trade |
| previous observed slot | 454315523 | predecessor event |
| derived pre (vb, vq) | (1045855315959624, 89309738191) | from `post − delta` |
| cache slots | [454315522, 454315524] | no state at 454315523 |
| `pre_state(454315524)` | none matching | no older cached state equals the pre |

The predecessor's post ≠ the trade's pre with no intermediate account state →
the true predecessor was not observed.

## 6. Root Cause

* `no_previous_event_observed` — **genuinely missing predecessor data**
  (first observation of the curve in the window). `CONFIRMED`.
* `previous_event_not_contiguous` — **missing predecessor event / non-event
  reserve mutation** (`INFERRED` / `HYPOTHESIS`), **not** a Neurone gate bug and
  **not** delivery lag.
* **No Neurone implementation bug** was found in this pass beyond the M3.2B
  fixes already shipped.

## 7. Fixes Applied

**None shipped.** The one candidate fix — deferred corroboration — was
implemented and measured, then reverted:

* replaced the single per-curve post anchor with a 32-deep post-state history
  (stricter `<=` slot, exact-equality corroboration), and
* deferred non-corroborated pump.fun SELL/token-target trades and retried them
  at end-of-run against the fully-populated event/account history.

Result: exclusion counts did **not** improve (they tracked live traffic), while
the harness grew materially more complex. Since the task requires minimal,
evidence-backed changes and forbids speculative fallbacks, the change was
reverted; `src/validate.rs` remains at the M3.2B gate.

## 8. Validation Results

All paths remain exactly as in M3.2B (no behaviour change was shipped):

| Protocol / Instruction | Exact | Unsupported | Parity |
|---|---:|---:|---:|
| PumpSwap SELL | all | 0 | 100% |
| PumpSwap `buy_exact_quote_in` | all | 0 | 100% |
| PumpSwap `buy` | all | 0 | 100% |
| Pump.fun `buy_exact_*` | all | 0 | 100% |
| Pump.fun SELL | all supported | ~329 | 100% of supported |
| Pump.fun token-target `buy` | all supported | ~330 | 100% of supported |

Mismatches: 0. Regressions: 0. 93 deterministic tests pass; clippy/fmt clean.

## 9. Remaining Exclusions

* First-observation curves (`no_previous_event_observed`): the harness starts
  mid-stream, so no predecessor exists. A production scanner that runs
  continuously before a trade occurs would have the predecessor; the exclusion
  is an artifact of a finite test window plus the fail-closed gate.
* Gaps between observed events (`previous_event_not_contiguous`): require the
  missing predecessor transaction (or a non-event mutation’s instruction) to
  corroborate. Obtaining that needs transaction-local capture of the missing
  transaction (instruction logs), which is evidence collection beyond this
  task’s fix boundary.

## 10. Verdict

**`PARTIALLY RESOLVED`**

The remaining exclusions are fully **classified** but not further reduced: one
class is a finite-window artifact (no predecessor observed) and the other is a
genuine gap in observed predecessor events (or a non-event reserve mutation).
No safe, evidence-backed Neurone fix was proven; the delivery-lag hypothesis was
tested and disproven; the fail-closed gate is preserved and no change was
shipped. M3.2B’s 60% reduction stands.

Stopping here per scope. Not proceeding to M4.
