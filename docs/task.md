# Task: Investigate Remaining Pump.fun SELL + Token-Target BUY Parity

## Objective

Investigate the remaining Pump.fun parity failures found in M3.1.

Current live results:

- Pump.fun `buy_exact_sol_in` / `buy_exact_quote_in`: 100% exact.
- PumpSwap SELL / `buy_exact_quote_in` / `buy`: 100% exact.
- Pump.fun SELL: ~80% exact.
- Pump.fun token-target `buy`: ~71% exact.

Do not assume the remaining failures are a formula problem.

The current evidence says the decoded event layout is internally consistent, but a significant subset of Pump.fun SELL/token-target BUY events do not satisfy the expected constant-product relationship. The bonding-curve account stream was added, but separate account/event arrival ordering only corroborates a subset of trades.

The purpose of this task is to determine the actual cause of those remaining failures.

This is an investigation task only.

Do not implement trading, Beam, TP/SL, capital allocation, or strategy logic.

Do not prematurely change production quote formulas.

---

## 1. Documentation First

Before investigating the data, thoroughly inspect the official first-party Pump.fun documentation and source material relevant to these failures.

At minimum inspect:

- official Pump program documentation
- official IDL
- `TradeEvent` definitions
- SELL documentation
- BUY documentation
- `buy` / token-target BUY documentation
- `buy_exact_*` documentation
- bonding-curve account definitions
- quote-mint / non-SOL support
- creator fees
- cashback
- holder rewards
- mayhem / related Pump program variants if officially documented
- fee-program documentation
- event/account layout evolution
- any official SDK/source implementing the formulas
- historical/breaking-change documentation relevant to TradeEvent layouts

Do not rely only on the previous reconciliation report.

The goal is to determine whether the official sources already explain any observed anomalous events.

For every relevant finding record:

- source
- file/document
- instruction/event/account
- exact field names
- field order if relevant
- formula if documented
- version/layout
- conditions under which the behavior applies

If official documentation is silent, explicitly state that.

Do not invent explanations from undocumented assumptions.

---

## 2. Read the Existing Investigation Carefully

Read:

- M2.2 reconciliation
- M3 Yellowstone parity report
- M3.1 Pump.fun state report
- existing Pump.fun decoder
- quote engine
- validation harness
- relevant regression tests

Preserve previous confirmed findings.

The known formulas should remain the baseline unless fresh ground truth disproves them.

---

## 3. Isolate the Failing Events

Create a clean dataset of:

### Pump.fun SELL

Separate:

- exact
- mismatch
- decoder error
- non-SOL/USDC
- other quote mint
- unknown variant
- same-slot cases
- multi-event transaction cases

### Pump.fun token-target BUY

Use the same classification.

For every failing event capture as much as possible:

```text
signature
slot
mint
bonding curve
quote mint
instruction
ix_name
event discriminator
all event fields
virtual token reserve
virtual quote reserve
real token reserve
real quote reserve
token amount
quote amount
fee
creator fee if present
transaction instruction ordering
inner instruction ordering
account updates
account slots
account data/version/length
other Pump-related events in transaction
```

Do not discard useful fields merely because they are not currently used by the quote engine.

---

## 4. Compare Exact vs Failing Events

Find structural differences between exact and failing events.

Compare:

- SOL vs USDC
- quote mint
- account layout length
- event payload length
- event field presence
- `ix_name`
- instruction variant
- fee bps
- creator fee
- cashback
- holder reward
- mayhem/variant flags
- bonding curve version
- transaction structure
- number of trades in transaction
- same-slot activity
- event ordering
- account update ordering
- reserve deltas
- real vs virtual reserves

Do not assume ordering is the cause. Find evidence.

---

## 5. Reconstruct Actual On-Chain State

For representative failures, reconstruct the trade as completely as possible.

RPC may be used only as an investigation/ground-truth tool. It must not become a production hot-path dependency.

For each representative failure determine:

1. Actual account state immediately before the trade.
2. Actual account state immediately after the trade.
3. Event state.
4. Instruction arguments.
5. Token/quote transfers.
6. Fee transfers.
7. Other Pump instructions/events in the same transaction.

Determine exactly which value differs.

The question is:

> What reserve state does the Pump.fun program actually use for this trade?

---

## 6. Investigate TradeEvent Semantics

Do not assume `TradeEvent.virtual_*` always means the same thing.

Determine from official source + empirical evidence whether the fields represent:

- pre-state
- post-state
- synthetic state
- accounting state
- state after fees
- state after another internal operation
- variant-dependent state

Test separately for:

- normal SOL Pump.fun
- non-SOL/USDC
- creator-fee variants
- cashback
- holder rewards
- mayhem/other official variants if applicable

If semantics differ by variant, document the exact rule.

---

## 7. Investigate Transaction Ordering

For failing transactions reconstruct:

```text
outer instruction
    ↓
inner instructions
    ↓
program invocation order
    ↓
events emitted
    ↓
account writes
```

Determine whether:

- multiple trades occur
- fees alter the curve between events
- buybacks occur
- rewards/cashback alter state
- another instruction mutates the curve
- events are emitted before/after relevant account writes

Do not simply label something a "multi-event transaction." Show what happened.

---

## 8. Investigate Same-Slot Ordering

Test:

- multiple transactions in one slot
- transaction ordering
- account update ordering
- Yellowstone transaction stream ordering
- Yellowstone account stream ordering

Determine whether slot alone is insufficient.

If a deterministic correlation key exists beyond slot, identify it.

If exact state cannot be reconstructed from Yellowstone alone, identify the exact missing information.

---

## 9. Investigate Program Variants

Specifically test whether failures correlate with:

- quote mint
- creator-fee configuration
- cashback
- holder rewards
- mayhem
- bonding curve version
- newer Pump program variants
- different instruction variants

Do not call something a variant without evidence from program/source/data.

---

## 10. Formula Revalidation

Only after understanding the failing-state semantics, compare:

### A — Current Neurone formula

### B — Formula explicitly documented by official Pump.fun sources

### C — Formula implemented in official SDK/source

### D — Formula derived directly from actual pre/post on-chain state

Determine whether:

- A = B = C = D
- A differs because state input is wrong
- formula is incomplete for a specific variant
- another documented formula is required

Do not modify production formulas merely to make samples pass.

---

## 11. Find the Minimum Correct Fix

Once the root cause is proven, determine the smallest correct architectural fix.

Possible outcomes:

- decoder variant handling
- event field interpretation
- transaction instruction correlation
- additional Yellowstone account filter
- account state history
- transaction-local state reconstruction
- program-variant classification
- additional event type
- explicit unsupported variant
- combination

Do not implement the final fix yet unless a tiny investigative change is required to prove the hypothesis.

The goal is to establish the correct design first.

---

## 12. Required Conclusions

For Pump.fun SELL and token-target BUY, answer explicitly:

1. What causes the mismatch?
2. Is the existing formula correct?
3. What state does the program actually use?
4. What does `TradeEvent` actually represent?
5. Why does the current Yellowstone event/state model fail?
6. Can exact state be reconstructed from Yellowstone alone?
7. If yes, what additional data/correlation is required?
8. If no, exactly what information is unavailable?
9. Does the issue affect only a variant/subset or the general Pump.fun path?
10. What is the minimum fix required?

---

## 13. Required Evidence

Do not conclude from aggregate percentages alone.

Provide representative examples for:

- at least 3 exact SELLs
- at least 3 failing SELLs
- at least 3 exact token-target BUYs
- at least 3 failing token-target BUYs

For each example show the values needed to reproduce the conclusion.

Where useful:

| Field | Exact sample | Failing sample | Interpretation |
|---|---:|---:|---|

---

## 14. Output

Write a technical investigation report:

1. Documentation Findings
2. Dataset / Sample Selection
3. Pump.fun SELL Investigation
4. Pump.fun Token-Target BUY Investigation
5. Exact vs Failing Event Comparison
6. TradeEvent Semantics
7. Transaction / Instruction Ordering
8. Account-State Correlation
9. Program Variants
10. Formula Reconciliation
11. Root Cause
12. Minimum Correct Fix
13. Yellowstone-Only Feasibility
14. Remaining Unknowns
15. Evidence Register

---

## 15. Status Rules

Use:

- `CONFIRMED` — directly proven by official source or ground-truth reconstruction
- `OBSERVED` — empirically observed but mechanism not formally established
- `INFERRED` — strongly supported interpretation
- `HYPOTHESIS` — plausible but not proven
- `UNRESOLVED` — insufficient evidence

Do not turn `INFERRED` or `HYPOTHESIS` into confirmed conclusions.

---

## Critical Rules

1. Documentation first.
2. Use official first-party Pump.fun sources wherever available.
3. Re-run the investigation; do not merely reinterpret old results.
4. Use real on-chain ground truth for representative failures.
5. Keep SOL and non-SOL quote pairs separate.
6. Keep Pump.fun instruction variants separate.
7. Keep official program variants separate when evidence supports them.
8. Do not change formulas before proving the root cause.
9. Do not use RPC in production architecture.
10. Do not implement execution.
11. Do not broaden into general Neurone architecture work.
12. Do not stop at "account stream lags."
13. Determine whether event/state semantics explain the mismatch.
14. If genuinely not reconstructable from Yellowstone, prove why.
15. End with a concrete conclusion and minimum next action.

## Completion Condition

The investigation is complete when we can confidently answer:

> Can Neurone quote Pump.fun SELL and token-target BUY exactly from Yellowstone, and if so, what exact state/correlation mechanism is required?

If yes, define the minimum implementation fix.

If no, define the precise protocol information Yellowstone does not expose and establish the correct exclusion boundary.

Do not implement the final fix as part of this investigation unless explicitly necessary for validation.

Write the report to the existing investigation/report directory.

Do not overwrite the Neurone blueprint.
