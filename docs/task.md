# NEURONE — State Mutation Handling & Stale-State Invalidation

## Objective

Implement the smallest production-safe change that allows Neurone to correctly handle **non-trade state mutations** discovered during M3.2.

M3.2 established that the remaining Pump.fun `previous_event_not_contiguous` exclusions are caused by:

- `SweepProtocolFee`
- `SweepCreatorFee`

These Pump.fun instructions adjust `virtual_quote_reserves` without emitting a `TradeEvent`.

This is legitimate protocol behavior, not malicious-token behavior.

However, the mutation materially changes the executable quote reserve. Therefore a trade armed against the pre-mutation state must not remain executable after the mutation.

The objective is NOT to force the old exclusion count to zero.

The objective is:

> **Make market state mutation-aware so that an abrupt quote-reserve mutation invalidates stale state and prevents execution until the affected market state is known/current again.**

Use the existing M3.2E report and current repository as the source of truth.

---

# Required Architectural Principle

Neurone must not assume:

```text
TradeEvent == every economically meaningful state change
```

The market state can evolve through:

```text
Trade
Reserve mutation
Trade
Lifecycle mutation
Trade
```

Any state-changing event that can affect execution must either:

1. update the market state deterministically, or
2. invalidate the market state until a fresh authoritative state is available.

Never silently continue using stale state.

---

# Critical Safety Invariant

A trade may execute ONLY when its state is:

```text
KNOWN
+
CURRENT
+
VALID
```

The following states must NOT permit execution:

```text
UNKNOWN
STALE
INVALIDATED
```

If a state-changing mutation occurs after a trade is armed:

```text
ARMED
  ↓
state mutation
  ↓
INVALIDATED
  ↓
NO FIRE
```

The system must re-establish valid current state before re-arming.

---

# Scope

Implement this narrowly.

The immediate mutation target is Pump.fun:

- `SweepProtocolFee`
- `SweepCreatorFee`
- program ID: `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P`
- observed discriminator: `742b4dbd117a482b`
- mutation: `virtual_quote_reserves`

M3.2E established that these instructions are delivered through Yellowstone, but the current decoder does not recognize them. The bonding-curve account stream can expose the resulting state, but reliable pre-next-trade ordering has not yet been proven.

Therefore:

**Do not assume the account stream is safe merely because it eventually contains the new state.**

---

# Phase 1 — Inspect Before Editing

First inspect the existing implementation.

Understand:

- market state structure
- state lifecycle/status
- Yellowstone transaction ingestion
- transaction ordering metadata
- instruction/event normalization
- Pump.fun decoding
- bonding-curve account cache
- validator
- arming logic
- capital/execution boundary if already present
- how stale/unknown state is currently represented
- current M3.2B gate

Do not refactor unrelated code.

Identify the smallest insertion point for mutation handling.

---

# Phase 2 — Establish Deterministic Event Ordering

Before implementing invalidation, determine what ordering information is already available.

Prefer an ordering key conceptually equivalent to:

```text
slot
+
transaction index/order
+
instruction index
```

The implementation does NOT need to invent fields that Yellowstone does not actually provide.

Determine what the current Yellowstone stream provides and use the strongest deterministic ordering available.

Same-slot events must not be treated as automatically unordered if deterministic transaction/instruction ordering is available.

If deterministic ordering is NOT available for a particular path, fail closed rather than guessing.

Document the ordering guarantee actually established by the implementation.

---

# Phase 3 — Represent the Sweep Mutation

Add the smallest representation necessary for the state machine to understand:

```text
SweepProtocolFee
SweepCreatorFee
```

The preferred conceptual model is:

```text
MarketEvent
├── Trade
└── ReserveMutation
```

But do NOT perform a broad event-system rewrite if a smaller implementation achieves the same safety invariant.

At minimum, the system must be able to associate the mutation with:

- affected bonding curve / market
- event position/order
- mutation type
- affected state field
- resulting state if deterministically known
- state validity after the mutation

---

# Phase 4 — Determine What Can Be Safely Applied

The M3.2E report states that the exact sweep instruction argument/account layout remains unresolved.

Therefore:

## Do NOT reverse-engineer or guess the sweep payload merely to eliminate exclusions.

If the exact resulting `virtual_quote_reserves` can be deterministically obtained from:

- a proven instruction format, OR
- an authoritative bonding-curve account update with a proven ordering guarantee,

then it may be applied.

Otherwise:

```text
mutation observed
      ↓
state cannot be deterministically reconstructed
      ↓
INVALIDATE / STALE
      ↓
NO EXECUTION
```

This is the preferred safe fallback.

---

# Phase 5 — State Invalidation

Implement explicit invalidation semantics if the architecture does not already have them.

Conceptually:

```text
CURRENT
   ↓
state-changing mutation
   ↓
STALE / INVALIDATED
   ↓
fresh authoritative state
   ↓
CURRENT
```

If an armed trade references the old state:

```text
armed_state_version != current_state_version
```

then:

```text
invalidate armed trade
```

Do not let the old trigger fire.

If the existing architecture already has an equivalent state-version mechanism, reuse it rather than adding another.

---

# Phase 6 — State Versioning

If necessary, add a lightweight per-market state version.

Conceptually:

```text
MarketState {
    version,
    slot/order,
    virtual_base,
    virtual_quote,
    ...
}
```

An armed opportunity records the state version it was created from:

```text
ArmedOpportunity {
    state_version,
    ...
}
```

Execution requires:

```text
armed.state_version == market.state_version
```

Any relevant state mutation increments or invalidates the version.

Do not over-engineer this.

A simple monotonic per-market version is preferable to a new persistence system.

---

# Phase 7 — Sweep Decoder

Only implement a sweep decoder if the exact format can be established from evidence already available in the repository or from reliable on-chain evidence.

Required confidence:

- discriminator confirmed
- instruction identity confirmed
- account/argument interpretation confirmed
- resulting state mutation confirmed

If the exact argument layout cannot be established safely:

**Do not guess.**

Instead, detect the known sweep instruction and invalidate the affected market.

This alone can be a correct safety improvement.

---

# Phase 8 — Account-State Reconciliation

If a sweep is detected and the account stream later provides the updated bonding-curve state:

```text
Sweep detected
   ↓
INVALIDATED
   ↓
account state update
   ↓
validate freshness/order
   ↓
CURRENT
```

Do not simply accept the first account update that arrives.

Verify:

- market identity
- slot/order
- freshness
- state consistency
- no later unprocessed mutation exists

If the update cannot be proven to be the correct post-mutation state:

remain invalidated.

---

# Phase 9 — Armed Trade Safety

This is the most important runtime behavior.

Test:

```text
Market current
↓
Strategy conditions pass
↓
ARM
↓
Sweep mutation
↓
ARM must become invalid
↓
Trigger arrives
↓
MUST NOT EXECUTE
```

Then:

```text
Fresh authoritative state
↓
Market current again
↓
Strategy evaluates again
↓
Can ARM again
↓
Trigger
```

The system must never execute using the pre-sweep quote reserve.

---

# Phase 10 — Do Not Treat the Mutation as Malicious

Do NOT create a safety rule such as:

```text
sweep detected → reject token
```

That would be incorrect.

The correct behavior is:

```text
sweep detected
→ invalidate stale state
→ refresh/reconcile
→ resume if state is valid
```

A legitimate Pump.fun fee sweep should not permanently blacklist a market.

---

# Phase 11 — Preserve Existing Fail-Closed Behavior

Unknown/stale state remains unsafe.

Do not:

- use stale reserves
- extrapolate missing reserves
- assume quote reserve did not change
- use the last known quote
- silently ignore the sweep
- weaken exact equality merely to increase support
- treat eventual account state as automatically ordered
- add RPC polling to the hot path

---

# Validation Requirements

Run all existing M3.2 tests and the full parity harness.

At minimum validate:

### Existing exact paths

- PumpSwap SELL
- PumpSwap `buy_exact_quote_in`
- PumpSwap `buy`
- Pump.fun exact-in BUY
- Pump.fun SELL supported paths
- Pump.fun token-target supported paths

All must remain:

```text
100% exact
0 mismatches
0 regressions
```

### New mutation tests

Create deterministic tests for:

1. Trade → Sweep → Trade
2. Trade → Sweep → no fresh state
3. Trade → Sweep → fresh state
4. ARM → Sweep → trigger
5. ARM → Sweep → refresh → re-arm → trigger
6. Same-slot sweep → trade where ordering is known
7. Same-slot case where ordering cannot be proven
8. Unknown sweep payload
9. Multiple mutations before refresh

Expected safety:

```text
Unknown/stale after mutation → NO EXECUTION
Fresh validated state → execution may resume
```

---

# Live Validation

If practical, run a live Yellowstone test specifically looking for:

```text
SweepProtocolFee
SweepCreatorFee
```

and capture:

- sweep slot
- transaction position
- instruction position
- curve
- pre-state
- post-state
- next trade
- account update position
- state-version transitions
- armed/invalidation behavior

Do not claim ordering guarantees from a small sample.

If the evidence is insufficient to prove a guarantee, document that and retain the conservative fallback.

---

# Performance Requirements

This is a hot-path system.

The implementation must remain:

- event-driven
- non-polling
- deterministic
- low allocation
- compatible with parallel market state
- compatible with sharding
- O(1) or effectively constant-time per relevant event where practical

Do not introduce:

- database access
- synchronous RPC calls
- blocking network calls
- global locks across all markets
- serial scanning
- heavyweight history reconstruction

A mutation for market A must not stall markets B–Z.

---

# Deliverable

Create:

`MILESTONE_3_3_STATE_MUTATION_HANDLING.md`

Document:

## 1. Implementation Summary

What changed and why.

## 2. State Model

How mutation, invalidation, state freshness and re-arming work.

## 3. Ordering Model

Exactly what event ordering guarantees are available and how they are used.

## 4. Sweep Handling

How `SweepProtocolFee` / `SweepCreatorFee` are detected and handled.

## 5. Safety Invariant

Demonstrate that stale armed trades cannot execute.

## 6. Tests

List all new deterministic tests and results.

## 7. Live Validation

Results from live Yellowstone mutation observation, if performed.

## 8. Performance

Any measurable overhead.

## 9. Remaining Limitations

Especially any unresolved ordering/account-stream guarantees.

## 10. Verdict

Choose:

- `IMPLEMENTED SAFELY`
- `IMPLEMENTED WITH CONSERVATIVE LIMITATION`
- `BLOCKED`

---

# Hard Scope Boundary

This task is ONLY about making Neurone's market state resilient to the non-trade Pump.fun quote-reserve mutation discovered in M3.2.

Do NOT:

- start M4 Strategy
- implement Beam
- implement final execution
- implement capital arbitration
- implement TP/SL
- add narrative/LLM logic
- build malicious-token scoring
- redesign Yellowstone
- redesign sharding
- add RPC polling
- refactor unrelated modules
- force the historical exclusion count to zero
- assume an undocumented instruction layout
- weaken the fail-closed safety boundary

The desired outcome is NOT:

> "Support every previously excluded trade."

The desired outcome is:

> **"If market state changes abruptly, Neurone notices it, invalidates stale state, and refuses to execute until the state is safely current again."**

Only once that invariant is proven should a market become eligible to re-arm and trade again.
