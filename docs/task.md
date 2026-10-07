# NEURONE — M3.2D: Transaction-Local Investigation of Remaining Exclusions

## Objective

Investigate the remaining Pump.fun exclusions from M3.2C by going one level deeper into the actual Yellowstone transaction data.

The question is no longer simply:

> Why can't Neurone correlate this state?

The question is:

> **What actually happened between the observed predecessor state and the excluded trade, and does that behavior reveal anything meaningful about the token or market?**

M3.2C established that the remaining exclusions are not explained by delivery lag and that no safe validator fix was proven. The remaining `previous_event_not_contiguous` cases require transaction-local evidence to determine whether the gap is caused by a missing TradeEvent, a non-TradeEvent reserve mutation, an unobserved program path, or another concrete state transition.

This task is an investigation first. Do not assume the tokens are malicious.

---

# Current Baseline

Use M3.2C as the baseline.

Remaining exclusion classes:

### 1. `no_previous_event_observed`

These are generally first observations of a curve within the finite test window.

M3.2C classified this as confirmed first-observation behavior.

Do NOT spend most of the investigation trying to turn this into a malicious-token detector.

Take a small representative sample only to confirm whether there is any unexpected behavior.

### 2. `previous_event_not_contiguous`

This is the primary target.

Current interpretation:

- previous observed event exists
- previous post-state does not equal current derived pre-state
- no intermediate account state was observed
- likely missing predecessor event or non-TradeEvent reserve mutation
- delivery lag was tested and disproven

The current explanation is still partly inferred/hypothesized and must now be tested using transaction-local evidence.

---

# Primary Questions

For representative excluded trades, determine exactly what happened between:

```text
previous observed event
        ↓
missing / unexplained transition
        ↓
excluded trade
```

Answer these questions:

1. Was there an intermediate transaction on the same curve?
2. Was there an intermediate trade?
3. Was it a Pump.fun trade that Neurone failed to attribute/decode?
4. Was it another program instruction that changed the curve?
5. Was it buyback/cashback/holder-reward/creator-related activity?
6. Was there a reserve mutation without a TradeEvent?
7. Was the transaction filtered out by the Yellowstone subscription?
8. Was the relevant instruction present but not decoded?
9. Was the relevant state changed by an unusual or unsupported instruction path?
10. Does the missing transition have any observable relationship to token safety or malicious behavior?

---

# Sample Selection

Do NOT inspect only one example.

Select representative samples from:

- Pump.fun SELL exclusions
- Pump.fun token-target BUY exclusions

Prefer several different mints/curves.

For each class, capture enough examples to determine whether the cause is consistent or heterogeneous.

Do not cherry-pick only easy cases.

---

# Required Transaction-Local Evidence

For each selected exclusion, retrieve/inspect the relevant transaction-local evidence available from Yellowstone and the existing project tooling.

Capture where available:

- mint
- slot
- transaction signature
- instruction ordering
- outer instructions
- inner instructions
- invoked programs
- account keys
- relevant bonding curve accounts
- relevant reserve accounts
- token accounts
- TradeEvent logs
- other program events/logs
- instruction arguments
- pre/post account state where available
- previous observed TradeEvent
- excluded TradeEvent
- state delta between previous observed post-state and current derived pre-state

Do not rely solely on aggregate counters.

---

# Trace the Missing State Transition

For each representative case, explicitly construct:

```text
Previous observed post-state
        ↓
        ?
        ↓
Current derived pre-state
```

Calculate the reserve delta:

```text
delta_virtual_base
delta_virtual_quote
delta_real_base
delta_real_quote
```

Then determine whether that delta corresponds to:

- another trade
- liquidity movement
- buyback
- cashback
- holder reward
- creator-related action
- virtual reserve update
- migration
- pool/curve transition
- account initialization/update
- another known Pump.fun instruction
- unknown instruction

If it corresponds to another trade, identify the transaction and instruction.

If it corresponds to a non-trade mutation, identify the instruction responsible.

If it cannot be determined, explicitly mark it `UNRESOLVED` rather than guessing.

---

# Program Attribution Investigation

M3.2C identified program-attribution gaps as one candidate explanation.

Test this directly.

Determine:

- whether the missing transaction invoked Pump.fun
- whether the relevant instruction was inside an inner instruction
- whether account filtering excluded it
- whether the event decoder ignored it
- whether it used a different program path/discriminator
- whether the transaction was present in Yellowstone but absent from Neurone's normalized event stream

If an attribution/decoder/filter bug is found, prove it with a concrete transaction before changing code.

---

# Non-Trade Mutation Investigation

Explicitly investigate whether the missing state transition can be produced without a TradeEvent.

Pay particular attention to any Pump.fun behavior already identified by the project research as capable of affecting curve/reserve state.

Do not assume these behaviors are malicious.

The question is:

> Does the instruction explain the exact state delta?

A theoretical possibility is not sufficient.

---

# Maliciousness / Safety Analysis

This section is REQUIRED.

For every confirmed exclusion cause, classify whether it is:

### A. Normal market behavior

Ordinary trading or documented protocol state transition.

### B. Unusual but legitimate protocol behavior

Unexpected from the normal trade path, but attributable to a legitimate Pump.fun instruction/mechanism.

### C. Unsupported / unknown behavior

The state changed but available evidence cannot identify the responsible instruction.

### D. Potentially suspicious behavior

There is concrete evidence of behavior that could indicate manipulation, malicious control, or an unsafe execution condition.

### E. Confirmed malicious behavior

Only use this classification if the evidence directly supports it.

**Do not label a token malicious merely because Neurone could not reconstruct its state.**

The investigation must distinguish:

```text
state-observability failure
        ≠
token safety failure
        ≠
malicious token
```

---

# Compare Excluded vs Supported Tokens

If practical, compare representative excluded cases against supported Pump.fun trades.

Look for concrete differences in:

- instruction path
- transaction structure
- programs invoked
- reserve mutations
- TradeEvent presence
- account updates
- market lifecycle
- token/curve state
- quote mint
- same-slot behavior

The goal is to determine whether excluded events form a meaningful behavioral class or are simply events that happen to lack sufficient predecessor evidence.

Do NOT invent a new safety heuristic from weak correlations.

---

# Fix Policy

This is investigation-first.

### If a real Neurone bug is proven:

Apply the smallest safe fix.

Examples:

- missing instruction attribution
- incorrect transaction filtering
- missed event decoding
- incorrect event normalization
- incorrect state correlation

### If protocol behavior is responsible:

Do not automatically modify the validator.

Determine whether that protocol behavior can be deterministically represented in the state model.

### If evidence is insufficient:

Keep `UnsupportedState`.

Do NOT:

- weaken exact state equality
- assume missing state
- create speculative fallback formulas
- use current account state as a substitute for historical pre-state
- add polling
- add an RPC dependency to the hot path
- accept unknown state as safe

Fail closed.

---

# Validation After Any Fix

Run the existing M3.2 parity harness.

Compare against M3.2C:

- exact
- unsupported
- mismatches
- errors
- `no_previous_event_observed`
- `previous_event_not_contiguous`
- Pump.fun SELL parity
- Pump.fun token-target parity
- all previously exact paths

Required:

- zero new formula mismatches
- zero regressions
- all existing 100% exact supported paths remain exact

If a fix increases supported trades but introduces mismatches, revert it and investigate.

---

# Deliverable

Create:

`MILESTONE_3_2D_TRANSACTION_LOCAL_EXCLUSION_INVESTIGATION.md`

Include:

## 1. Executive Summary

What actually causes the remaining exclusions.

## 2. Sample Set

List the representative transactions/mints investigated and why they were selected.

## 3. Transaction-Local Findings

For each sample, show the state transition and responsible instruction/transaction where identifiable.

## 4. Root-Cause Classification

Separate:

- missing TradeEvent
- unobserved transaction
- program attribution/filter issue
- decoder issue
- non-TradeEvent reserve mutation
- legitimate protocol behavior
- unresolved

## 5. Excluded vs Supported Comparison

Explain whether excluded events form a meaningful behavioral class.

## 6. Maliciousness / Safety Findings

Explicitly state whether the evidence shows:

- normal behavior
- unusual legitimate behavior
- unknown behavior
- potentially suspicious behavior
- confirmed malicious behavior

Do not overstate.

## 7. Fixes Applied

Only evidence-backed production changes.

## 8. Validation

Before/after metrics and regression results.

## 9. Remaining Unknowns

State exactly what cannot be determined from the available Yellowstone evidence.

## 10. Verdict

Choose:

- `RESOLVED`
- `PARTIALLY RESOLVED`
- `BLOCKED`

---

# Hard Scope Boundary

Do NOT:

- start M4 Strategy
- implement Beam
- implement execution
- implement capital arbitration
- implement TP/SL
- redesign Yellowstone architecture
- redesign state sharding
- add narrative/LLM logic
- add speculative safety scoring
- add RPC polling to the hot path
- refactor unrelated modules
- manufacture a malicious-token filter
- force the exclusions to zero

The objective is **forensic understanding of the missing state transitions**.

If the evidence shows that the exclusions are ordinary/legitimate protocol behavior or simply insufficient historical observation, document that clearly.

If the evidence reveals a real Neurone bug, fix it minimally.

If the evidence reveals genuinely suspicious behavior, document the exact evidence and do not generalize beyond what the data supports.
