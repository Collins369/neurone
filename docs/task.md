# Task: M3.1 — Complete Pump.fun Yellowstone Trade-Time State

## Objective

Fix the remaining Pump.fun Yellowstone state problem discovered in M3.

M3 proved that the quote formulas themselves are correct, but Pump.fun SELL and token-target `buy` cannot currently be guaranteed exact from the transaction/event stream alone for a significant subset of live events because the event's reconstructed reserves are not always the true trade-time pre-state.

The M3 report states that the required fix is:

> add a Pump.fun bonding-curve **account subscription through Yellowstone** and correlate it with the transaction/event stream, or implement a version-aware `TradeEvent` decoder if that is sufficient.

The goal of this task is to make Pump.fun market state sufficiently complete that Neurone can continuously scan and track Pump.fun markets without relying on RPC polling in the hot path.

Do not redesign Neurone.

Do not implement trading/execution.

Do not reopen the entire protocol-parity investigation.

---

# 1. Source of Truth

Use the existing M3 report:

`MILESTONE_3_YELLOWSTONE_PARITY_REPORT.md`

Important M3 findings:

- PumpSwap is already 100% exact through Yellowstone:
  - SELL: 10,683/10,683
  - `buy_exact_quote_in`: 6,523/6,523
  - `buy`: 2,435/2,435
- Pump.fun exact-in BUYs are 528/528 exact.
- Pump.fun SELL and token-target `buy` remain state-dependent.
- The formula is not the identified problem.
- The live event stream is insufficient for the true trade-time state in the affected cases.
- M3 explicitly identifies the Pump.fun bonding-curve account stream as the required next direction.
- M3 also identified foreign-event collisions and decoder/layout issues; preserve those fixes.

Do not undo working M3 fixes.

---

# 2. First Inspect the Existing Code

Before changing anything, inspect:

- `src/ingest/solami.rs`
- `src/events.rs`
- `src/decode/pumpfun.rs`
- `src/decode/mod.rs`
- `src/quote.rs`
- `src/engine.rs`
- `src/shard.rs`
- `src/config.rs`
- existing Yellowstone subscription filters
- existing tests
- M3 validation harness
- M3 report

Understand exactly how transaction updates and account updates are currently handled.

Reuse the existing Yellowstone connection and subscription infrastructure.

Do not create a second gRPC client.

---

# 3. Determine the Correct Yellowstone State Source

Before implementing the correlation logic, establish exactly which Pump.fun account(s) contain the required bonding-curve state.

The minimum required state is the state used by the already validated Pump.fun formulas:

- virtual token/base reserve
- virtual quote/SOL reserve
- any required fee-related state
- mint association
- bonding-curve identity/address
- account version/layout where relevant

Use the existing protocol documentation/reconciliation work and the current decoder.

If multiple Pump.fun account layouts exist, identify them explicitly.

Do not assume one fixed byte layout for every historical account.

---

# 4. Add Pump.fun Bonding-Curve Account Subscription

Extend the existing Yellowstone subscription so the runtime receives the relevant Pump.fun bonding-curve account updates.

Requirements:

- Yellowstone gRPC only for the production/hot path
- no RPC polling
- no REST polling
- reuse existing connection/authentication
- maintain current transaction/event subscriptions
- account updates must be filtered as narrowly as practical
- do not subscribe indiscriminately to every Solana account if a Pump.fun-specific filter is available

Document the exact Yellowstone account filter used.

---

# 5. Build a Bonding-Curve State Cache

Create a deterministic in-memory state cache keyed by the bonding-curve account/mint identity.

For each active Pump.fun bonding curve, retain only the state required for:

- market filtering
- exact quote calculation
- state validation
- trade-time correlation

At minimum, the state record should contain:

```text
bonding_curve_address
mint
virtual_token_reserves
virtual_quote_reserves
real_token_reserves
real_quote_reserves
fee-related fields if required
account_version/layout
slot
write/update ordering information
```

Use exact integer types.

No floating point.

Avoid unnecessary allocations and locks in the hot path.

The cache must support many markets concurrently.

---

# 6. Correlate Account State With Transaction Events

This is the core of the task.

For every Pump.fun trade event:

1. Identify the mint/bonding curve.
2. Identify the relevant slot/update ordering.
3. Obtain the correct trade-time bonding-curve state.
4. Determine whether the event reserves are post-trade and reconstruct pre-state where appropriate.
5. Validate:

```text
pre_state + trade_delta = post_state
```

where the protocol semantics require it.

Do not simply use the latest account state.

Do not simply use the event's reserve fields when they are known to be unreliable.

Do not race a newer account update against an older transaction.

The correlation logic must respect Solana/Yellowstone update ordering and slots.

---

# 7. Handle Multiple Trades in the Same Transaction

M3 specifically observed that multi-event transactions can make naive:

```text
pre = event_post - event_delta
```

reconstruction unreliable.

Therefore test transactions containing:

- one Pump.fun trade
- multiple Pump.fun trades
- multiple relevant events
- multiple instructions in one transaction
- transactions involving other programs

For multi-event transactions, determine whether the true pre-state can be reconstructed from:

- ordered transaction events
- account update state
- instruction ordering
- event fields
- or a combination

Do not assume one event is independent of another.

---

# 8. Account Layout / Version Handling

M3 found that Pump.fun account/event layouts can vary.

Implement explicit layout/version handling where required.

Do not create a fragile decoder that assumes one fixed account length forever.

For unknown layouts:

```text
UNKNOWN_LAYOUT
```

must be rejected safely.

Never silently decode unknown bytes as a known version.

Add tests for every layout currently supported by the project/evidence.

---

# 9. Re-run Pump.fun Live Parity

After implementing the account stream and correlation:

run the live Yellowstone validation again.

At minimum measure:

### Pump.fun SELL

Target:

**100% exact for valid/reconstructable events.**

### Pump.fun `buy_exact_sol_in`

Must remain:

**100% exact.**

### Pump.fun `buy_exact_quote_in`

Must remain:

**100% exact.**

### Pump.fun token-target `buy`

Target:

**100% exact for valid/reconstructable events.**

The existing M3 formula for token-target `buy` remains:

```text
sol = ceil(
    virtual_quote_reserve * token_amount
    /
    (virtual_base_reserve - token_amount)
)
```

Do not change the formula unless new ground-truth evidence proves it wrong.

---

# 10. Yellowstone-Only Classification

Update the M3 classification.

The desired result is:

| Path | Desired classification |
|---|---|
| Pump.fun SELL | YELLOWSTONE-ONLY EXACT |
| Pump.fun `buy_exact_sol_in` | YELLOWSTONE-ONLY EXACT |
| Pump.fun `buy_exact_quote_in` | YELLOWSTONE-ONLY EXACT |
| Pump.fun token-target `buy` | YELLOWSTONE-ONLY EXACT |
| PumpSwap SELL | YELLOWSTONE-ONLY EXACT |
| PumpSwap `buy_exact_quote_in` | YELLOWSTONE-ONLY EXACT |
| PumpSwap `buy` | YELLOWSTONE-ONLY EXACT |

If any path cannot achieve this, do not fake the result.

State precisely:

- what state is missing
- why Yellowstone does not provide it
- whether the limitation is decoder, filter, ordering, layout, or actual protocol observability

---

# 11. Preserve the Scanner Architecture

This is not just a parity test.

The resulting state feed must be usable by the future Neurone scanner.

The intended flow is:

```text
                 YELLOWSTONE
                      │
             ┌────────┴────────┐
             ↓                 ↓
       Transactions         Accounts
             │                 │
             └────────┬────────┘
                      ↓
              Event Normalizer
                      ↓
              Market Identity
                      ↓
              State Correlation
                      ↓
             Parallel Sharded State
                      ↓
                Market Filters
```

Do not turn the system into:

```text
receive token
→ finish token
→ scan next token
```

Many Pump.fun and PumpSwap markets must be trackable simultaneously.

Reuse the existing deterministic shard architecture.

Do not add an unnecessary database to the hot path.

---

# 12. State Freshness

The cache must distinguish:

- current state
- stale state
- unknown state

A stale bonding-curve state must not be used for an exact quote.

Define deterministic rules for:

```text
KNOWN
STALE
UNKNOWN
```

Include slot/update metadata so the quote engine knows whether the state is safe to use.

Do not silently fall back to an older state.

---

# 13. Performance

Measure the added account stream/correlation cost.

Report:

- account updates/sec
- transaction updates/sec
- correlation latency
- state-cache update latency
- quote latency
- p50
- p95
- p99
- memory usage if practical
- lock/contention behavior

The account stream must not destroy the low-latency characteristics demonstrated by M3.

Keep telemetry asynchronous.

---

# 14. Regression Tests

Add deterministic tests covering:

1. single Pump.fun trade
2. multiple Pump.fun trades in one transaction
3. event + account state correlation
4. correct trade-time state selection
5. stale account state rejection
6. unknown layout rejection
7. supported account layouts
8. Pump.fun SELL exact parity
9. Pump.fun token-target BUY exact parity
10. existing Pump.fun exact-in BUY parity
11. existing PumpSwap parity
12. foreign-event collision protection
13. account-update ordering
14. same-slot update handling where relevant

All existing tests must remain green.

---

# 15. No Scope Creep

Do NOT implement:

- Beam
- transaction submission
- automatic trading
- TP/SL
- capital arbitration
- strategy logic
- narrative analysis
- social analysis
- LLM logic
- database persistence
- RPC polling fallback

This task is strictly:

**Pump.fun Yellowstone state completeness + correlation + parity.**

---

# 16. Required Final Report

Create a technical report covering:

## 1. Problem Confirmed

What exactly caused the M3 Pump.fun state-dependent failures?

## 2. Account Source

Which Pump.fun account(s) were subscribed to and why?

## 3. Yellowstone Subscription

Exact account filter and subscription design.

## 4. State Cache

State structure, keying, freshness, and update ordering.

## 5. Correlation Algorithm

Explain exactly how transaction/event updates are correlated with account updates.

Include multi-event transaction handling.

## 6. Layout Handling

Supported Pump.fun account versions/layouts.

## 7. Live Parity Results

Include:

- samples
- exact
- mismatches
- errors
- classification

For every Pump.fun instruction.

## 8. Regression Results

Existing total test count and new tests.

## 9. Performance

Latency and throughput.

## 10. Scanner Readiness

State whether Pump.fun markets can now be continuously tracked in the parallel scanner without RPC polling.

## 11. Remaining Limitations

Only real limitations.

## 12. Final Status

Use exactly one:

- `PASS`
- `PASS WITH EXCLUSIONS`
- `FAIL`

The target is:

**PASS**

if all supported Pump.fun paths achieve exact Yellowstone-only parity.

---

# Completion Condition

The task is complete when Neurone can do:

```text
Yellowstone
    ↓
Pump.fun transaction/event
    +
Pump.fun bonding-curve account state
    ↓
correct trade-time state
    ↓
exact quote
    ↓
on-chain parity
```

for Pump.fun SELL and token-target BUY, while preserving the already-passing Pump.fun exact-in and all PumpSwap paths.

At that point, Neurone should have a reliable Yellowstone-derived market-state foundation suitable for the next master build stage:

**Discovery → deterministic filters/safety → pre-arm**

Do not proceed into execution after this task.

Write the final report to the existing investigation/report location.

Do not overwrite the Neurone blueprint.
