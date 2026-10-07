# NEURONE — M3.2C Remaining Unsupported-State Investigation

## Objective

Continue the M3.2 exclusion investigation **surgically**.

M3.2B reduced Pump.fun exclusions from **1,654 → 659** and proved that the dominant earlier failures were validation-gate correlation bugs. The remaining 659 samples are still safely excluded, but they have not yet been fully explained.

The goal is to determine **why each remaining exclusion class occurs**, and fix only causes that are demonstrably caused by Neurone's implementation.

Do **not** redesign the architecture or move to M4 until this investigation is complete.

---

## Current State

M3.2B final live run:

- Pump.fun SELL: 1,400 exact, 329 unsupported
- Pump.fun token-target BUY: 876 exact, 330 unsupported
- Total remaining unsupported: **659**
- `no_previous_event_observed`: **202**
- `previous_event_not_contiguous`: **457**
- Supported paths remained 100% exact.
- Mismatches: **0**
- Regressions: **0**
- Tests: **93 passing**
- No quote/fee/decoder changes were required.
- M3.2B verdict: **PARTIALLY RESOLVED**

M3.2B explicitly states that the remaining 659 require further investigation rather than being assumed to be protocol limitations.

---

# Investigation Scope

Investigate ONLY these two remaining classes:

1. `no_previous_event_observed` — 202
2. `previous_event_not_contiguous` — 457

The central question is:

> Are these remaining exclusions caused by missing/late Yellowstone data, an incomplete validation correlation model, same-transaction mutation, an actually missed intermediate TradeEvent, or some other concrete Neurone implementation issue?

Do not assume the answer beforehand.

---

# Required Investigation

## 1. Reproduce and capture representative exclusions

Run the existing M3.2B parity harness and collect representative examples from BOTH classes.

For each sample capture, where available:

- mint
- slot
- transaction signature
- instruction name
- TradeEvent data
- derived pre-state
- derived post-state
- previous observed event
- previous event slot
- previous event pre-state
- previous event post-state
- bonding-curve account cache state
- account-update slot
- virtual token reserve
- virtual quote reserve
- real token reserve
- real quote reserve
- quote mint
- layout/version
- all relevant Yellowstone transaction/account updates around the sample

Do not inspect only aggregate counters.

---

## 2. Investigate `no_previous_event_observed`

For representative samples determine which of these actually happened:

### A. First trade genuinely visible in the run window
There is no earlier TradeEvent available because the test window started after the curve already existed.

### B. Earlier TradeEvent existed but Neurone failed to observe it
Look for evidence in Yellowstone data, transaction ordering, filtering, subscription behavior, or event decoding.

### C. Earlier account state existed but arrived too late
Determine whether the account stream could have provided the required pre-state but did not arrive before the transaction/event correlation.

### D. A matching older account state was available but validation failed to use it
If so, identify the exact implementation bug.

### E. Same-transaction or adjacent instruction mutation
Determine whether the curve changed in the same transaction without a usable predecessor TradeEvent.

Do not label something a protocol limitation unless the evidence actually supports that conclusion.

---

## 3. Investigate `previous_event_not_contiguous`

This is the larger and more important class.

For representative samples determine exactly why the previous observed event is not the immediate predecessor.

Check:

- Was a TradeEvent genuinely missed?
- Was there another transaction between the two observed events?
- Was the intermediate transaction filtered out?
- Was it decoded under another instruction/program path?
- Did the curve mutate without a TradeEvent?
- Did account updates reveal an intermediate state?
- Did multiple trades occur in the same slot?
- Did instruction ordering affect the observed sequence?
- Did Yellowstone delivery/order create an apparent gap?
- Was the gap caused by a validation implementation assumption?

For every representative sample, attempt to classify the gap into a concrete cause.

---

# Important Safety Rule

Do NOT weaken the corroboration gate merely to increase the exact count.

The existing M3.2B rule is intentionally fail-closed:

> UnsupportedState remains the result whenever the required pre-state cannot be independently corroborated.

Only change the validator if evidence proves the current validator is rejecting a state that can be safely and deterministically established from existing Yellowstone data.

No speculative fallback.

---

# Fix Rules

If a concrete Neurone bug is found:

1. Fix the smallest possible component.
2. Prefer `src/validate.rs` or the directly responsible correlation/ingestion code.
3. Do not alter validated quote formulas unless investigation proves a formula defect.
4. Do not change fee math.
5. Do not change protocol decoding unless evidence proves decoding is responsible.
6. Do not add RPC polling to the hot path.
7. Do not introduce a database.
8. Do not redesign the parallel architecture.
9. Do not add M4 strategy behavior.
10. Preserve fail-closed behavior.

If no safe fix is proven, leave the sample UnsupportedState and document why.

---

# Required Validation

After any fix:

### Run the same M3.2B parity harness

Compare:

- total samples
- exact
- unsupported
- mismatches
- errors
- `no_previous_event_observed`
- `previous_event_not_contiguous`
- any newly introduced rejection reason
- account corroboration count

### Regression requirement

All previously exact paths must remain exact.

Required:

- PumpSwap SELL: no regression
- PumpSwap exact quote BUY: no regression
- PumpSwap token-target BUY: no regression
- Pump.fun exact-in BUY: no regression
- Pump.fun SELL supported samples: no regression
- Pump.fun token-target supported samples: no regression

Any new mismatch is a regression and must be investigated before declaring success.

---

# Deliverable

Create:

`MILESTONE_3_2C_REMAINING_UNSUPPORTED_INVESTIGATION.md`

Include:

## 1. Executive Summary

What happened to the remaining 659 exclusions.

## 2. Before/After

Exact counts before and after any fix.

## 3. `no_previous_event_observed` Analysis

Break down the 202 samples by actual cause.

## 4. `previous_event_not_contiguous` Analysis

Break down the 457 samples by actual cause.

## 5. Representative Evidence

Show concrete examples and the relevant state/slot/event relationships.

## 6. Root Cause

Clearly distinguish:

- Neurone implementation bug
- Yellowstone delivery/timing behavior
- genuinely missing predecessor data
- same-transaction mutation
- protocol behavior
- unresolved/insufficient evidence

Do not use “protocol limitation” as a catch-all.

## 7. Fixes Applied

List only evidence-backed changes.

## 8. Validation Results

Provide the complete before/after parity table.

## 9. Remaining Exclusions

For anything still unsupported, explain precisely why it remains unsupported.

## 10. Verdict

Choose exactly one:

- **RESOLVED**
- **PARTIALLY RESOLVED**
- **BLOCKED**

---

# Hard Scope Boundary

This task is ONLY the investigation and safe resolution of the remaining M3.2 unsupported-state exclusions.

Do NOT:

- start M4 Strategy
- implement Beam
- implement execution
- implement capital arbitration
- implement TP/SL
- redesign state sharding
- introduce new architecture
- add narrative/LLM logic
- add polling
- optimize unrelated code
- refactor unrelated modules
- change the blueprint

If the evidence shows the remaining exclusions are genuinely unresolvable with the current Yellowstone evidence, stop and document that result.

The objective is not to force 100% support.

The objective is to know **exactly why the remaining exclusions happen and whether Neurone can safely resolve them.**
