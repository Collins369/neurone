# NEURONE — M3.2E: Final Virtual-Quote Mutation Investigation

## Objective

Perform the **final, narrowly scoped investigation** into the remaining M3.2 Pump.fun exclusions.

M3.2D established a strong and consistent forensic pattern:

- `previous_event_not_contiguous` exclusions have:
  - `Δvirtual_base = 0`
  - `Δvirtual_quote ≠ 0`
- This is a quote-reserve-only mutation.
- It cannot be an ordinary buy or sell because those move both base and quote.
- It is consistent with a `virtual_quote_reserves` adjustment that does not emit the TradeEvent used by Neurone.
- It is not currently evidence of malicious tokens.
- `no_previous_event_observed` is a finite test-window first-observation artifact.
- No Neurone bug has been proven.
- No production change has been shipped.
- The current fail-closed M3.2B gate remains unchanged.

The one unresolved question is:

> **What exact transaction/instruction performs the quote-only `virtual_quote_reserves` mutation, and can Neurone safely observe or decode it?**

This is the final investigation before deciding whether M3.2 can be closed.

---

# Source of Truth

Use these existing reports and the current repository implementation:

- `MILESTONE_3_2B_UNSUPPORTED_STATE_INVESTIGATION.md`
- `MILESTONE_3_2C_REMAINING_UNSUPPORTED_INVESTIGATION.md`
- `MILESTONE_3_2D_TRANSACTION_LOCAL_EXCLUSION_INVESTIGATION.md`
- Current `src/validate.rs`
- Current Pump.fun decoders/state model
- Existing Yellowstone transaction/account subscriptions
- Existing project protocol research and tests

Do not overwrite historical reports.

Create a new report:

`MILESTONE_3_2E_FINAL_VIRTUAL_QUOTE_MUTATION_INVESTIGATION.md`

---

# Primary Question

Identify the exact on-chain mechanism responsible for:

```text
previous observed post-state
        ↓
virtual_base unchanged
virtual_quote changed
        ↓
next TradeEvent
```

Specifically determine:

1. Which transaction performs the mutation?
2. Which instruction performs it?
3. Which program owns the instruction?
4. What are the instruction arguments?
5. What accounts are modified?
6. Does it emit an event?
7. If it emits an event, what discriminator/layout does it use?
8. Does it update the Pump.fun bonding-curve account?
9. Does Yellowstone account streaming expose the resulting state?
10. Can the mutation be deterministically reconstructed before the next trade?
11. Can Neurone safely incorporate it into its state model?

---

# Investigation Method

## 1. Start from a known excluded sample

Use the M3.2D examples, especially the captured `AF3r22z5...` curve and its relevant slots.

For a selected excluded trade:

```text
previous observed TradeEvent
        ↓
excluded trade
```

Determine the exact state delta.

Then work **backwards and forwards through transaction history** to locate the transaction that caused:

```text
Δvirtual_base = 0
Δvirtual_quote != 0
```

Do not infer the instruction solely from documentation.

Find the actual transaction.

---

# 2. Capture the Mutating Transaction

For every successfully identified mutation, record:

- slot
- transaction signature
- mint
- bonding curve address
- invoked program(s)
- outer instruction index
- inner instruction index, if applicable
- instruction discriminator
- instruction name, if identifiable
- instruction arguments
- account metas
- pre/post bonding curve account state
- pre/post virtual quote reserve
- pre/post virtual token reserve
- real reserves
- emitted logs
- emitted events
- transaction ordering relative to the surrounding trades

The report must contain at least several concrete examples from different curves if available.

Do not rely on a single transaction.

---

# 3. Identify the Exact Instruction

Potential candidates may include:

- `set_virtual_quote_reserves`
- creator/holder-reward accounting
- buyback accounting
- another Pump.fun instruction
- another program entirely

Treat these only as candidates.

The task is to prove the actual instruction from transaction evidence.

If the instruction cannot be identified, mark it:

`UNRESOLVED`

Do not force a match.

---

# 4. Determine Why the Current Yellowstone Event Model Misses It

Once the mutation is identified, trace it through Neurone's current pipeline:

```text
Solana
 ↓
Yellowstone
 ↓
subscription filter
 ↓
normalization
 ↓
decoder
 ↓
state update
 ↓
validator
```

Determine exactly where the mutation disappears.

Possible outcomes:

### A. Yellowstone does not provide it

Document the exact limitation.

### B. Yellowstone provides it but current subscription filters exclude it

Identify the filter.

### C. Yellowstone provides it and the transaction is present, but Neurone ignores the program/instruction

Identify the code path.

### D. Neurone receives the instruction but does not decode it

Identify the missing decoder.

### E. Neurone decodes it but does not apply the state update

Identify the missing state transition.

### F. Account stream provides the post-state but too late for the current single-pass validator

Measure this rather than assuming it.

### G. Something else

Document precisely.

---

# 5. Determine Whether It Can Be Safely Integrated

This is critical.

Do NOT automatically implement a fix just because the instruction has been found.

Determine whether the mutation can be represented deterministically.

Ask:

- Is the resulting virtual quote reserve directly encoded?
- Is the delta directly encoded?
- Can it be reconstructed from instruction arguments?
- Is there a reliable event?
- Is there a reliable account update?
- Does it occur before the next trade?
- Can Yellowstone ordering guarantee that Neurone sees it before the next trade?
- Can same-slot ordering be handled deterministically?
- Is there any ambiguity between multiple mutations?
- Can a stale/missing mutation cause a false accepted trade?

The standard is:

> **If Neurone cannot guarantee the correct pre-state, it must remain UnsupportedState.**

---

# 6. Safety / Maliciousness Re-check

This is NOT a token-scoring task.

Confirm whether the exact identified mutation is:

- normal documented protocol behavior
- unusual but legitimate protocol behavior
- an administrative/control-plane action
- an unsupported behavior
- potentially suspicious
- malicious

Only classify as malicious if the transaction evidence actually supports that conclusion.

The existing M3.2D conclusion was:

> state-observability limitation, not token-safety finding.

Do not reverse that conclusion without direct evidence.

Also determine whether **the ability to manipulate `virtual_quote_reserves` itself has any safety implications for trading**, even if the mechanism is legitimate.

For example:

- Can it materially change quoted execution?
- Could it make a stale pre-state dangerous?
- Should Neurone invalidate an armed trade after such a mutation?
- Is the mutation expected by the protocol's economics?

Separate:

```text
legitimate protocol mechanism
```

from:

```text
safe to trade through without observing it
```

These are NOT necessarily the same thing.

---

# 7. Decide Whether a Production Fix Is Justified

### If a deterministic, reliable fix is proven:

Implement the **smallest possible change**.

Prefer:

- targeted instruction decoding
- targeted state update
- targeted Yellowstone filter extension

Do not redesign the architecture.

### If the mutation can only be observed unreliably:

Do NOT ship a fix.

Keep the fail-closed exclusion.

### If the account stream is sufficient but timing is not guaranteed:

Do NOT assume it is safe merely because the final account state eventually arrives.

### If evidence is incomplete:

Do NOT guess.

Leave `UnsupportedState`.

---

# 8. Validation

If a production fix is implemented:

Run the full existing M3.2 parity harness.

Compare:

- total samples
- supported
- unsupported
- exact
- mismatches
- errors
- Pump.fun SELL
- Pump.fun token-target BUY
- PumpSwap SELL
- PumpSwap exact quote BUY
- PumpSwap `buy`
- Pump.fun exact-in BUY

Required:

- no new mismatches
- no regressions
- all previously exact paths remain exact

Specifically measure whether the quote-only exclusions decrease.

Report:

```text
M3.2B baseline
M3.2D baseline
M3.2E result
```

If no production fix is justified, do not modify production behavior.

---

# Final Deliverable

Create:

`MILESTONE_3_2E_FINAL_VIRTUAL_QUOTE_MUTATION_INVESTIGATION.md`

Use this structure:

## 1. Executive Summary

State exactly what the mutation is and whether its instruction was identified.

## 2. Concrete Transaction Evidence

Provide several real transaction examples.

## 3. Exact Instruction

Program, instruction, discriminator, arguments, accounts and event/log behavior.

## 4. State Transition

Show:

```text
before
→ mutation
→ after
```

with virtual base/quote and relevant real reserves.

## 5. Why Neurone Misses It

Identify the exact point in the current pipeline where the information is lost.

## 6. Safety Analysis

Separate:

- protocol legitimacy
- execution safety
- token maliciousness

## 7. Production Fix Decision

Choose:

- **SAFE TO IMPLEMENT**
- **NOT SAFE TO IMPLEMENT**
- **INSUFFICIENT EVIDENCE**

Explain why.

## 8. Implementation

If and only if safe, document the minimal production change.

## 9. Validation

Full before/after parity and exclusion counts.

## 10. Remaining Unknowns

Anything that still cannot be proven.

## 11. Final Verdict

Choose:

- `RESOLVED`
- `PARTIALLY RESOLVED`
- `BLOCKED`

---

# Hard Scope Boundary

This is the **final M3.2 exclusion investigation**.

Do NOT:

- start M4 Strategy
- implement Beam
- implement execution
- implement capital arbitration
- implement TP/SL
- redesign Yellowstone
- redesign sharding
- add narrative/LLM logic
- add speculative safety scoring
- add RPC polling to the hot path
- rewrite unrelated code
- force exclusions to zero
- assume every quote mutation is malicious
- accept an unverified state as safe

The goal is not:

> "Make the unsupported count disappear."

The goal is:

> **Identify exactly what causes the quote-only state transition, determine whether Neurone can observe it deterministically, and only then decide whether it is safe to integrate.**

If the evidence proves that the current fail-closed boundary is the correct design, say so clearly and close M3.2 without forcing a code change.
