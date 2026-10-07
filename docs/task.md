# Task: M3 — Yellowstone Live Protocol Parity Test

## Objective

Now that the Pump.fun + PumpSwap documentation/reconciliation investigation is complete, test the **confirmed quote/parity paths directly through Solami Yellowstone gRPC**.

This task is about validating the complete live data path:

```text
Solami Yellowstone gRPC
        ↓
live account/event updates
        ↓
decoder
        ↓
trade-time state
        ↓
protocol + instruction identification
        ↓
exact integer quote engine
        ↓
compare against actual on-chain trade result
        ↓
telemetry + latency measurement
```

Do **not** reopen the previous documentation investigation unless the Yellowstone test produces a genuine contradiction.

Do **not** implement trading/execution yet.

Do **not** redesign the Neurone architecture.

---

# 1. Source of Truth

Use the completed Pump.fun/PumpSwap reconciliation report as the protocol-parity baseline.

The report established the following production-certifiable quote paths:

### Pump.fun

1. SELL
2. `buy_exact_sol_in`
3. `buy_exact_quote_in`

### PumpSwap

1. SELL
2. `buy_exact_quote_in`
3. `buy` when the quote inflow required by the formula is observable

The reconciliation also established:

- PumpSwap effective quote reserve:

```text
q_eff = raw_quote_reserve + event.virtual_quote_reserves
```

- Pump.fun exact-in BUY:

```text
tokens_out =
floor(
    virtual_base_reserve * (input - 1)
    /
    (virtual_quote_reserve + input - 1)
)
```

- PumpSwap exact-in BUY:

```text
base_out =
floor(
    base_reserve * (quote_input - 1)
    /
    (effective_quote_reserve + quote_input - 1)
)
```

- PumpSwap SELL:

```text
quote_out =
floor(
    effective_quote_reserve * base_input
    /
    (base_reserve + base_input)
)
```

Use the exact formulas and state semantics established by the reconciliation. Do not replace them with floating-point approximations.

---

# 2. First Inspect Existing Neurone Implementation

Before writing code:

1. Inspect the current Neurone repository.
2. Identify:
   - Yellowstone client
   - subscription code
   - protobuf/IDL definitions
   - normalizer
   - market-state engine
   - sharding
   - existing quote engine
   - existing Pump.fun/PumpSwap decoders
   - existing tests
   - telemetry/performance infrastructure
3. Determine what M1/M2/M2.1 already implemented.
4. Reuse existing architecture where appropriate.

Do not duplicate an existing Yellowstone client or quote engine.

Do not create a parallel architecture merely for this task.

---

# 3. Yellowstone Connection Test

Connect to the existing Solami Yellowstone gRPC endpoint/configuration already used by Neurone.

Verify:

- connection succeeds
- authentication works
- subscription remains alive
- updates are continuously received
- reconnect behavior works if already implemented
- no polling is introduced into the hot path

Use the project's existing environment/configuration conventions.

Do not hardcode credentials.

Do not print credentials or tokens into logs.

---

# 4. Live Yellowstone Data Path

Build or complete the smallest path necessary to observe the relevant Pump.fun and PumpSwap state/events directly from Yellowstone.

The hot path should be:

```text
Yellowstone update
    ↓
decode
    ↓
normalize
    ↓
identify protocol/instruction
    ↓
obtain required trade-time state
    ↓
quote
```

Keep this deterministic.

Do not introduce:

- LLMs
- narrative analysis
- social data
- REST polling
- sequential token scanning
- unnecessary RPC requests
- database writes in the hot path

Async telemetry may run alongside the hot path.

---

# 5. Pump.fun Yellowstone Validation

Test the following independently.

## 5.1 Pump.fun SELL

For every usable observed SELL:

1. Decode the relevant bonding curve state.
2. Determine the correct trade-time pre-state.
3. Decode the trade/event.
4. Calculate the expected gross quote output.
5. Apply the correct observed/applicable fee state.
6. Compare with the actual transaction/event result.

Record:

- signature
- mint
- instruction
- relevant reserves
- input amount
- fee bps
- calculated output
- observed output
- difference
- exact/non-exact classification

Target:

**100% exact for valid ground-truth samples.**

---

## 5.2 `buy_exact_sol_in`

Validate:

```text
tokens_out =
floor(
    virtual_base_reserve * (sol_in - 1)
    /
    (virtual_quote_reserve + sol_in - 1)
)
```

Ensure the `sol_in` value and reserve state correspond to the correct trade-time state.

Target:

**100% exact.**

---

## 5.3 `buy_exact_quote_in`

Validate the corresponding exact-quote-input path using the reconciled integer formula and fee semantics.

Target:

**100% exact.**

---

# 6. PumpSwap Yellowstone Validation

## 6.1 Effective Quote Reserve

This is a critical test.

For every relevant PumpSwap event:

```text
effective_quote_reserve =
raw_quote_reserve +
event.virtual_quote_reserves
```

Verify that the **event's trade-time virtual reserve** is used rather than a later/current account value.

Explicitly test historical/nonzero virtual-reserve cases.

The previous M2.2B `(20,5)` issue was resolved by using the event-time value. Make sure the live Yellowstone implementation does not regress this.

---

## 6.2 PumpSwap SELL

Validate:

```text
quote_out =
floor(
    effective_quote_reserve * base_in
    /
    (base_reserve + base_in)
)
```

Test across the observed dynamic fee regimes:

- `(25,5)`
- `(2,93)`
- `(20,5)`

Target:

**100% exact for valid ground-truth samples.**

---

## 6.3 PumpSwap `buy_exact_quote_in`

Validate the exact-input formula:

```text
base_out =
floor(
    base_reserve * (quote_input - 1)
    /
    (effective_quote_reserve + quote_input - 1)
)
```

Target:

**100% exact.**

---

## 6.4 PumpSwap `buy`

Validate the token-target/base-output direction separately.

Do not accidentally treat it as `buy_exact_quote_in`.

If the quote inflow can be reconstructed exactly from Yellowstone-observable state, validate the resulting quote/output.

If a required input cannot be reconstructed from Yellowstone alone, document the exact missing state instead of inventing it.

---

# 7. Ground Truth

Ground truth must come from the actual Solana transaction/event/account state.

For each test:

```text
Yellowstone-observed inputs
        ↓
Neurone quote
        ↓
actual on-chain result
```

Compare integer values exactly.

Do not use approximate percentage error as the primary parity test.

A difference of `1` lamport/token unit is still a mismatch unless the protocol formula explicitly permits that rounding behavior.

---

# 8. Yellowstone-Only Requirement

Determine explicitly which quote paths can be reconstructed from Yellowstone alone.

For each path classify:

- `YELLOWSTONE-ONLY EXACT`
- `YELLOWSTONE-ONLY BUT STATE-DEPENDENT`
- `REQUIRES RPC`
- `NOT CURRENTLY RECONSTRUCTABLE`

The goal is to maximize the first category.

Do not quietly fall back to RPC.

If RPC is used for **ground truth validation only**, clearly separate it from the production/hot path.

---

# 9. Parallelism Requirement

This test must preserve Neurone's intended architecture.

Do not implement:

```text
receive token A
→ finish A
→ receive token B
→ finish B
```

Instead verify that independent markets can be processed concurrently:

```text
Yellowstone
    ↓
normalizer
    ↓
hash(pool/mint)
    ↓
parallel shards
 ├── market A
 ├── market B
 ├── market C
 └── market N
```

The quote calculation itself should remain deterministic and extremely small.

Measure whether decoding/state updates or locking become bottlenecks.

---

# 10. Latency Measurement

Measure at least:

1. Yellowstone message arrival
2. decode start/end
3. normalization
4. state update
5. protocol/instruction identification
6. quote calculation
7. ground-truth comparison where applicable

Report:

- p50
- p95
- p99
- throughput
- error/drop rate

Do not contaminate hot-path latency measurements with disk/database logging.

Telemetry should be asynchronous where possible.

---

# 11. Required Test Matrix

Create a matrix similar to:

| Protocol | Instruction | State Source | Formula | Samples | Exact | Mismatch | Yellowstone-only |
|---|---|---|---|---:|---:|---:|---|
| Pump.fun | SELL | Yellowstone | ... | ... | ... | ... | ... |
| Pump.fun | buy_exact_sol_in | Yellowstone | ... | ... | ... | ... | ... |
| Pump.fun | buy_exact_quote_in | Yellowstone | ... | ... | ... | ... | ... |
| PumpSwap | SELL | Yellowstone | ... | ... | ... | ... | ... |
| PumpSwap | buy_exact_quote_in | Yellowstone | ... | ... | ... | ... | ... |
| PumpSwap | buy | Yellowstone | ... | ... | ... | ... | ... |

Also include dynamic fee regimes where relevant.

---

# 12. Failure Classification

Every mismatch must be classified.

Use:

- decoder error
- wrong account layout
- wrong instruction identification
- wrong trade-time state
- wrong fee state
- wrong reserve semantics
- wrong event interpretation
- formula error
- rounding error
- multi-event transaction ambiguity
- missing Yellowstone field/state
- genuine unexplained protocol behavior

Do not simply report "quote mismatch."

---

# 13. Regression Protection

Add deterministic tests for every parity rule that is successfully validated.

Especially protect:

- Pump.fun `-1`
- PumpSwap `-1`
- PumpSwap signed virtual quote reserves
- event-time virtual reserve versus current account reserve
- dynamic fee regimes
- exact integer arithmetic

Do not use floating point.

Do not remove existing tests.

Run the existing relevant test suite after changes.

---

# 14. Production-Code Boundary

This task is a **live Yellowstone validation milestone**, not the final trading engine.

Do NOT implement:

- automatic trade execution
- Beam submission
- TP/SL execution
- capital allocation
- autonomous strategy changes

The output should prove that Neurone can observe and calculate correctly from Yellowstone before execution is added.

---

# 15. Deliverable

Produce a detailed technical report after the test.

Include:

## 1. Existing Architecture Used

What M1/M2/M2.1 components were reused.

## 2. Yellowstone Connection

Endpoint/configuration, subscription type, filters, connection behavior.

Do not expose credentials.

## 3. Live Data Path

Exact path from Yellowstone update to quote result.

## 4. Pump.fun Results

Separate results for:

- SELL
- `buy_exact_sol_in`
- `buy_exact_quote_in`

## 5. PumpSwap Results

Separate results for:

- SELL
- `buy_exact_quote_in`
- `buy`

## 6. Yellowstone-Only Assessment

Clearly identify which paths are truly reconstructable without RPC.

## 7. Exact Parity Matrix

Include sample counts, exact counts, mismatches, and mismatch classifications.

## 8. Latency / Throughput

Include p50/p95/p99 and throughput.

## 9. Problems Found

Only actual problems discovered during the live test.

## 10. Changes Made

List code/tests/config changes precisely.

## 11. Final M3 Status

Classify:

- `PASS`
- `PASS WITH EXCLUSIONS`
- `FAIL`

Explain why.

---

# Critical Rules

1. Do not reopen the already-completed documentation investigation unless Yellowstone produces contradictory evidence.
2. Do not blindly trust the old implementation.
3. Do not blindly trust the old report either; validate it through live Yellowstone.
4. Do not change formulas merely because a sample mismatches.
5. Trace mismatches to state, decoding, fees, instruction semantics, or formula before changing anything.
6. Keep Pump.fun and PumpSwap logic separate.
7. Keep each instruction variant separate.
8. Use integer arithmetic only.
9. Never use floating point for protocol quotes.
10. Never silently fall back to RPC in the production path.
11. RPC may be used only as an explicit ground-truth/reference source during validation.
12. Do not introduce unnecessary architecture.
13. Preserve Neurone's parallel/sharded design.
14. Do not implement trading/execution yet.
15. If all validated paths pass, stop. Do not create another open-ended investigation.

## Completion Condition

The task is complete when we have empirically demonstrated:

**Solami Yellowstone → live decode → trade-time state → exact protocol quote → on-chain parity**

for every supported path that can be reconstructed from Yellowstone, with explicit exclusions for anything that cannot.

Write the final report to the repository's existing investigation/report location. Do not overwrite the source blueprint.
