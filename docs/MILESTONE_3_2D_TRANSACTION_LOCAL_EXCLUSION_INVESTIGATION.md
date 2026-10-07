# Neurone — M3.2D: Transaction-Local Exclusion Investigation

Investigation only. No production change shipped (M3.2B gate unchanged).

Statuses: `CONFIRMED`, `OBSERVED`, `INFERRED`, `HYPOTHESIS`, `UNRESOLVED`.

---

## 1. Executive Summary

Transaction-local forensics found a **single, consistent signature** for the
`previous_event_not_contiguous` exclusions: between the observed predecessor
trade and the excluded trade, the curve's **virtual token reserve is unchanged
(`Δvirtual_base = 0`) while the virtual quote reserve changes
(`Δvirtual_quote ≠ 0`)**. That is a **quote-reserve-only mutation** — it cannot
be produced by a buy or sell (both move base and quote). It is therefore a
documented-family `virtual_quote_reserves` adjustment that emits no
`TradeEvent`, so the event stream alone cannot reconstruct the trade's
pre-state.

This is not a missed trade, not a program-attribution/decoder bug, and not
evidence of malicious behaviour. It is a legitimate protocol state transition
that the current event-only state model cannot see.

`no_previous_event_observed` is confirmed as first-observation-of-curve inside
the finite test window.

No Neurone implementation bug was found in this pass; the fail-closed
`UnsupportedState` boundary is retained and no change was shipped.

## 2. Sample Set

Representative exclusions captured with signature, curve, predecessor slot,
trade slot and the signed reserve delta (`src/validate.rs` bounded capture
during the M3.2D run):

* `previous_event_not_contiguous` (SELL and token-target), several distinct
  mints/curves: `AF3r22z5…`, `BPaamNLK…`, `F34EfqTo…` (and others).
* `no_previous_event_observed` (a small sample only, per scope): `BE1ZxH2g…`,
  `AF3r22z5…`, `Cu689NsV…`, …
* RPC transaction-local traces for `AF3r22z5Nuz4wo2dRiE2EAt9dw7z984VQ736gpHtJ9k7`
  (slots 454320420–454320508).

## 3. Transaction-Local Findings

Captured deltas (`Δvirtual_base = prev_post_vb − derived_pre_vb`,
`Δvirtual_quote = derived_pre_vq − prev_post_vq`):

| Curve | prev slot | slot | Δvirtual_base | Δvirtual_quote |
|---|---:|---:|---:|---:|
| AF3r22z5… | 454320425 | 454320426 | **0** | −1,105,467,489 |
| AF3r22z5… | 454320428 | 454320428 | **0** | −500,144,534 |
| BPaamNLK… | 454320428 | 454320429 | **0** | +128,590,866 |
| F34EfqTo… | 454320426 | 454320430 | **0** | −15,378,358,686 |

**Every** `previous_event_not_contiguous` sample has `Δvirtual_base == 0` with
a non-zero quote delta. `CONFIRMED` pattern.

RPC trace of `AF3r22z5…` shows the curve's trades each emit
`TradeEvent (bddb7fd3…)` plus a companion event `31487b2d…` — which is **not**
a pump.fun event (it does not match any pump/pump_amm anchor discriminator) and
is emitted by a co-invoked program (`MAyhSmzX…` / the pump fee program
`pfeeUxB6…`). Program attribution correctly ignores it; it is not the mutation.

## 4. Root-Cause Classification

* `no_previous_event_observed` — **genuinely missing predecessor data**
  (first observation of the curve in the window). `CONFIRMED`.
* `previous_event_not_contiguous` — **non-`TradeEvent` reserve mutation:
  `virtual_quote_reserves`-only** (`Δvirtual_base = 0`). `INFERRED` (pattern is
  unambiguous across all sampled cases; the specific instruction is not yet
  pinned).
* **Not** a missing TradeEvent (`CONFIRMED` — a missed trade would move base).
* **Not** a program-attribution / filtering / decoder bug (`CONFIRMED` — the
  companion event is a different program's and is correctly ignored).
* **Not** delivery lag (`CONFIRMED` in M3.2C).

## 5. Excluded vs Supported Comparison

* Supported trades: predecessor post-state equals the trade's derived
  pre-state exactly (`Δ = 0` on both sides).
* Excluded trades: identical decode, identical instruction names, identical
  layout fingerprints — the only difference is the **quote-only mutation**
  inserted between two trades.
* The excluded set therefore forms a **meaningful behavioural class**: trades
  that follow a `virtual_quote_reserves` adjustment. It is not a random or
  cherry-picked subset.

## 6. Maliciousness / Safety Findings

Classification: **B — unusual but legitimate protocol behaviour**
(`INFERRED`). The evidence is a systematic quote-reserve adjustment consistent
with the documented `virtual_quote_reserves` mechanism (the pump IDL contains
`set_virtual_quote_reserves`, and the docs describe appended/effective quote
reserves). There is **no** evidence of manipulation, malicious control or an
unsafe execution condition; the tokens are ordinary (valid mints, normal trade
paths, standard Token-2022 base program, fee-program co-invocation).

Explicitly: this is a **state-observability limitation**, not a token-safety
finding and not a malicious-token classification.

## 7. Fixes Applied

**None shipped.** No Neurone bug was proven. The one candidate — deferred
corroboration against the account stream — was tested in M3.2C and did not
reduce exclusions, so it remains reverted. The M3.2B gate is unchanged and
fail-closed.

The bounded forensic capture used to obtain §3's evidence was **temporary
instrumentation** in `src/validate.rs` and has been reverted; the log output is
preserved in this report. The only committed change is this report.

A safe future fix would require the `virtual_quote_reserves` mutation to be
either (a) delivered as an observable bonding-curve account update before the
next trade (which the current single-pass harness did not achieve), or (b)
decoded from the mutating instruction itself. Both are outside this
investigation's fix boundary and neither was demonstrated to be reliable.

## 8. Validation

No change was shipped, so parity is identical to M3.2B:

* All supported paths 100% exact; **0 mismatches**; **0 regressions**.
* 93 deterministic tests pass; clippy/fmt clean; release build ok.
* Unsupported counts unchanged (~25–30% of pump.fun SELL/token-target trades,
  traffic-dependent).

## 9. Remaining Unknowns

* The **specific instruction** that mutates `virtual_quote_reserves` is
  `UNRESOLVED`. Candidates: `set_virtual_quote_reserves`, creator/holder-reward
  or buyback accounting. Pinning it needs the *mutating* transaction (not the
  trade transaction), which was not in the captured window.
* Whether the bonding-curve account stream reliably delivers that mutation as an
  update before the next trade in a continuous (non-windowed) production
  scanner (`HYPOTHESIS`: it should, because it is an account write).

## 10. Verdict

**`PARTIALLY RESOLVED`**

The remaining exclusions are now **forensically explained at the state
transition level**: a quote-only `virtual_quote_reserves` mutation between
consecutive trades (legitimate protocol behaviour), plus first-observation
curves. No Neurone implementation bug was found; the fail-closed boundary is
retained; no speculative change was shipped. Fully resolving them requires
observing or decoding the mutating instruction, which is beyond this task's
fix boundary.

Stopping here per scope. Not proceeding to M4.
