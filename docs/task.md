# Neurone — M3.2B: Pump.fun Unsupported-State Investigation

## Objective

Investigate the **1,654 Pump.fun `UnsupportedState` samples** produced by M3.2A and determine precisely why they failed pre-state corroboration.

This is a focused investigation and fix task.

Required sequence:

```text
Excluded event
    ↓
Investigate why it was excluded
    ↓
Identify the concrete failure mechanism
    ↓
Implement the smallest justified fix
    ↓
Rerun the same parity test
    ↓
Measure the reduction in exclusions
```

Do not broaden the scope. Do not redesign the architecture or start M4.

## Source of Truth

Use:

`MILESTONE_3_2A_FEE_ARITHMETIC_AND_STATE_GATE_REPORT.md`

M3.2A established:

- 17,991 / 17,991 supported samples exact.
- 1,654 samples classified as `UnsupportedState`.
- 893 Pump.fun SELL exclusions.
- 761 Pump.fun token-target BUY exclusions.
- Zero formula/fee mismatches.
- Zero decode/errors.

The exclusions are the subject of this task.

## 1. Investigate the Exclusions First

Select representative `UnsupportedState` events from both:

- Pump.fun SELL
- Pump.fun token-target `buy`

Do not immediately modify production logic.

For each representative case, determine exactly why the corroboration gate rejected it.

Inspect the existing evidence available to the system:

- transaction signature
- slot
- instruction ordering
- ix_name
- TradeEvent
- previous observed event on the same curve
- derived pre-state
- previous event post-state
- bonding-curve account-cache state
- account-cache slot
- virtual token reserves
- virtual quote reserves
- real token reserves
- real quote reserves
- quote mint
- layout/version
- relevant Yellowstone account/transaction updates

## 2. Classify the Actual Failure

Determine which concrete mechanism caused each exclusion.

Possible causes include, but are not limited to:

- no previous event was available;
- previous event existed but was not considered contiguous;
- account state existed but arrived too late;
- account state existed but slot correlation rejected it;
- state-cache correlation bug;
- event ordering/correlation issue;
- same-transaction state mutation;
- reserve mutation outside the TradeEvent;
- another specific, reproducible state-reconstruction issue.

Do not assume one of these causes is correct before inspecting evidence.

The purpose is to discover the actual cause.

## 3. Do Not Change the Formula

M3.2A already established:

```text
formula/fee mismatch = 0
decode/error = 0
```

Do not alter the validated quote formulas unless the investigation produces direct evidence of a separate formula defect.

Do not re-open the completed fee-arithmetic work.

## 4. Fix Only the Proven Cause

Once the exclusion mechanism is established:

- implement the smallest change necessary to fix that mechanism;
- preserve the existing corroboration safety principle;
- do not weaken `UnsupportedState` into an assumption;
- do not classify an event as supported merely to increase coverage;
- do not introduce speculative fallback logic.

A previously unsupported event may become supported only when its pre-state can be deterministically established.

## 5. Preserve Existing Exact Paths

The fix must not regress:

- PumpSwap SELL
- PumpSwap `buy_exact_quote_in`
- PumpSwap token-target `buy`
- Pump.fun `buy_exact_sol_in`
- Pump.fun `buy_exact_quote_in`
- already-supported Pump.fun SELL
- already-supported Pump.fun token-target BUY
- Yellowstone ingestion
- parallel sharded state processing
- existing state-cache behavior
- official fee primitives

## 6. Rerun the Same Parity Harness

After the targeted fix, rerun the same live Yellowstone parity test used by M3.2A.

Report:

| Protocol / Instruction | Samples | Exact | Mismatch | Errors | Unsupported | Parity |
|---|---:|---:|---:|---:|---:|---:|

Also report:

```text
UnsupportedState before
UnsupportedState after
UnsupportedState reduction
newly-supported samples
remaining mismatches
```

The goal is to determine whether the identified fix actually resolves the investigated exclusions.

## 7. Validate the Fix

For every newly-supported category/sample:

- verify exact quote parity;
- verify the pre-state is genuinely corroborated;
- verify no unsupported state was silently promoted;
- verify the result remains deterministic.

If the fix resolves only one class of exclusions, report that clearly.

Do not claim all exclusions are solved unless the evidence demonstrates it.

## 8. If Multiple Causes Are Found

Keep the scope narrow.

For each discovered cause:

```text
Cause
→ Evidence
→ Minimal fix
→ Validation result
```

Do not start unrelated investigations.

If a remaining exclusion has a different cause that cannot be safely fixed within this task, leave it as `UnsupportedState` and document the concrete reason.

## Hard Constraints

- Investigate before modifying production behavior.
- Fix only evidence-backed causes.
- No broad refactor.
- No architecture redesign.
- No M4 Strategy implementation.
- No Beam.
- No execution engine.
- No TP/SL.
- No capital-arbitration work.
- No LLM/narrative logic.
- No RPC polling added to the hot path.
- No speculative protocol formulas.
- No weakening of safety guarantees.
- Integer quote math remains deterministic.
- Keep the change minimal and auditable.

## Final Deliverable

Produce a concise investigation report containing:

1. Number of exclusions examined.
2. Pump.fun SELL exclusions examined.
3. Token-target BUY exclusions examined.
4. Exact reason(s) they failed.
5. Evidence supporting each reason.
6. Minimal fix implemented.
7. Before/after parity results.
8. Number of exclusions removed.
9. Remaining exclusions and their known reason.
10. Any regressions.
11. Final verdict:

```text
RESOLVED
PARTIALLY RESOLVED
or
BLOCKED
```

Do not proceed to M4 as part of this task.

The task ends after the exclusion investigation, targeted fix, and parity rerun.
