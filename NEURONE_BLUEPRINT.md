# NEURONE — Project Blueprint

**Status:** Source of truth for implementation  
**Project:** Neurone  
**Repository:** `/home/neurone` (target; coding agent may establish the repo path)  
**Language:** Rust  
**Primary real-time data layer:** Solami Yellowstone gRPC  
**Primary transaction transport:** Solami Beam  
**Frontend:** Deferred to a later architecture/design pass  
**Architecture principle:** Parallel, event-driven, deterministic, latency-sensitive  
**Relationship to Xion:** Completely standalone. Neurone does not inherit Xion's six-layer architecture or terminology.

---

# 1. Purpose

Neurone is a lightweight autonomous Solana meme-market execution system.

Its job is to:

1. observe Solana continuously;
2. maintain thousands of independent market states in parallel;
3. identify deterministic technical setups;
4. perform deterministic safety/executability checks;
5. pre-arm a trade before the final trigger;
6. wait for a precise on-chain condition;
7. execute immediately when that condition occurs;
8. exit according to deterministic TP/SL rules;
9. measure actual execution performance.

Neurone is deliberately **not** an AI trading agent.

There is:

- no LLM in the trading path;
- no narrative interpretation;
- no discretionary reasoning;
- no autonomous strategy rewriting;
- no heavyweight database in the hot path;
- no serial scan → analyze → decide → trade pipeline.

The core idea is:

> **READ FAST → DECIDE EARLY → ARM → WAIT → FIRE → LAND**

---

# 2. Core Architectural Principle

Neurone must be understood as a **parallel collection of continuously running state machines**, not as a sequential trading loop.

A Yellowstone event arrives.

The system determines which pool/token state it belongs to.

That state is updated independently.

Other tokens continue processing at the same time.

Conceptually:

```text
                       YELLOWSTONE
                            │
                            ▼
                    ┌───────────────┐
                    │ Event Ingest  │
                    └───────┬───────┘
                            │
              ┌─────────────┼─────────────┐
              ▼             ▼             ▼
          Shard 0       Shard 1       Shard N
              │             │             │
        ┌─────┴─────┐ ┌─────┴─────┐ ┌─────┴─────┐
        │ Token A   │ │ Token D   │ │ Token X   │
        │ Token B   │ │ Token E   │ │ Token Y   │
        │ Token C   │ │ Token F   │ │ Token Z   │
        └───────────┘ └───────────┘ └───────────┘
              │             │             │
              └─────────────┼─────────────┘
                            ▼
                    FILTER / ARM ENGINE
                            │
                ┌───────────┴───────────┐
                ▼                       ▼
          NOT QUALIFIED               ARMED
                                        │
                                        ▼
                              WAIT FOR TRIGGER
                                        │
                                        ▼
                                CAPITAL ARBITER
                                        │
                             ┌──────────┴──────────┐
                             │                     │
                          NO SLOT              SLOT FREE
                             │                     │
                           ignore                  ▼
                                               EXECUTE
                                                  │
                                                  ▼
                                                 EXIT
                                                  │
                                                  ▼
                                           ASYNC TELEMETRY
```

The **market-processing architecture remains parallel even if V1 uses only one active $20 capital position at a time**.

That capital restriction is an execution constraint, not a system architecture constraint.

---

# 3. Design Goals

## Primary

- extremely low processing latency;
- deterministic behavior;
- high event throughput;
- parallel state processing;
- minimal synchronization;
- minimal allocation;
- predictable execution;
- safe failure behavior;
- measurable latency from chain event to transaction landing;
- simple enough to audit.

## Secondary

- easy replay/testing;
- configurable thresholds;
- observable state;
- clean separation between hot path and cold path;
- future frontend compatibility.

## Explicit non-goals for V1

- LLM trading;
- narrative scoring;
- social-media analysis;
- portfolio optimization;
- multi-strategy portfolio management;
- complex machine learning;
- generic blockchain support;
- historical backtesting infrastructure;
- frontend implementation;
- unnecessary Solami products;
- premature ShredDirect integration;
- elaborate database architecture.

---

# 4. Solami Integration Policy

Neurone should use only Solami infrastructure that materially contributes to the system.

## Required

### Yellowstone gRPC

Primary real-time chain ingestion.

Used for:

- account updates;
- transaction updates;
- program activity;
- relevant pool/token state;
- real-time market events;
- deterministic state reconstruction.

Yellowstone is the primary source of truth for the live market state.

### Beam

Primary transaction submission/transport layer.

Used for:

- priority transaction sending;
- fast leader-directed submission;
- transaction landing feedback;
- execution telemetry.

The execution path should terminate at Beam rather than routing through unnecessary public RPC infrastructure.

## Supporting

### RPC / Comet

Use only where Yellowstone alone does not provide an efficient answer.

Potential uses:

- initial state hydration;
- targeted account/state verification;
- fast token-account/program queries;
- startup recovery;
- transaction confirmation/reconciliation where needed.

RPC/Comet must not become a polling loop in the hot path.

## Conditional

### Solami on-chain helpers / SDK

Use only if they reduce implementation complexity without creating meaningful hot-path overhead.

Neurone should retain control over transaction construction where latency matters.

## Not V1

### ShredDirect

Do not add initially.

Only introduce it if instrumentation proves Yellowstone latency is the dominant bottleneck after the complete system is optimized.

### Blur

Do not add initially.

Local deterministic decoding/state reconstruction should be preferred if feasible.

Blur may be evaluated later if local decoding becomes an unnecessary engineering burden.

### Mirage

Not needed for the Rust backend.

### Index Engine

Not needed for live execution.

Could later support research/backtesting/historical analytics.

### Webhooks

Not appropriate for the persistent low-latency market stream.

### Edge Network

Not required for the first system.

---

# 5. System Pipeline

The canonical Neurone pipeline is:

```text
CHAIN
  ↓
YELLOWSTONE
  ↓
EVENT NORMALIZATION
  ↓
PARALLEL STATE UPDATE
  ↓
DETERMINISTIC FILTER
  ↓
SAFETY / EXECUTABILITY CHECK
  ↓
TRADE ARMING
  ↓
WAIT FOR TRIGGER
  ↓
CAPITAL ARBITRATION
  ↓
TRANSACTION FINALIZATION
  ↓
BEAM
  ↓
LANDING
  ↓
POSITION STATE
  ↓
TP / SL TRIGGER
  ↓
BEAM
  ↓
EXIT LANDING
  ↓
ASYNC TELEMETRY
```

The important distinction is:

**Filtering and arming happen before the critical trigger.**

At trigger time, Neurone should not be asking:

> "Should I trade?"

That decision should already have been made.

At trigger time it should effectively be asking:

> "Did the armed condition become true?"

If yes:

> Fire.

---

# 6. Runtime Components

Neurone should initially be composed of the following logical components.

## 6.1 Ingestion Engine

Responsibilities:

- maintain Yellowstone connection;
- subscribe to required Solana programs/accounts;
- receive updates;
- validate stream messages;
- normalize relevant events;
- attach slot/block metadata;
- forward events to the correct state shard.

Requirements:

- reconnect automatically;
- detect stale streams;
- handle malformed/unexpected updates safely;
- avoid blocking downstream processing;
- preserve ordering where ordering is required.

The ingestion layer should not contain trading logic.

---

# 7. Event Normalization

Raw Yellowstone updates should be converted into compact internal events.

Example conceptual event types:

```text
TokenCreated
PoolCreated
PoolStateChanged
Swap
LiquidityChanged
TokenAccountChanged
MintAuthorityChanged
FreezeAuthorityChanged
TransactionObserved
SlotUpdate
BlockBoundary
```

Only events required by the current strategy should survive into the hot path.

The system should avoid carrying large raw protobuf objects through every processing stage if a compact normalized representation is sufficient.

---

# 8. Parallel Market State

This is the heart of Neurone.

Every relevant token/pool has an independent state object.

Example:

```rust
MarketState {
    mint,
    pool,
    liquidity,
    reserve_a,
    reserve_b,
    price,
    volume_5m,
    volume_previous_window,
    volume_acceleration,
    buy_route_valid,
    sell_route_valid,
    safety_status,
    market_cap_estimate,
    state,
    armed_trade,
    last_slot,
    timestamps,
}
```

The exact structure should be determined during implementation after identifying the minimum data required by the strategy.

Do not over-model the market.

---

# 9. State Sharding

The market-state map must be partitioned.

Conceptually:

```text
hash(pool_or_mint) → shard
```

Each shard owns its state.

This avoids a single global lock.

Example:

```text
Shard 0
  ├── Pool A
  ├── Pool K
  └── Pool Q

Shard 1
  ├── Pool B
  ├── Pool M
  └── Pool R

Shard 2
  ├── Pool C
  ├── Pool N
  └── Pool S
```

Events for a given pool should normally be processed by the same shard so that local ordering is preserved.

The target architecture is:

> **many independent state machines + minimal synchronization**

not:

> one global market state protected by locks.

---

# 10. Market State Machine

Every market instance follows a deterministic lifecycle.

```text
OBSERVING
    │
    ├── fails hard filter ──► REJECTED
    │
    ▼
QUALIFIED
    │
    ├── safety/executability fails ──► REJECTED
    │
    ▼
ARMED
    │
    ├── expires / invalidates ──► OBSERVING
    │
    ▼
TRIGGERED
    │
    ▼
ENTRY SUBMITTED
    │
    ├── failed ──► ENTRY_FAILED
    │
    ▼
POSITION OPEN
    │
    ├── TP ──► EXIT_SUBMITTED
    ├── SL ──► EXIT_SUBMITTED
    └── invalidated ──► EXIT_SUBMITTED
    │
    ▼
CLOSED
    │
    ▼
TELEMETRY
```

A rejected market should not repeatedly consume work unless its state changes enough to justify reevaluation.

---

# 11. Deterministic Strategy V1

V1 should use a deliberately simple technical strategy.

Candidate requirements:

1. market/pool is supported;
2. liquidity exceeds configured minimum;
3. 5-minute volume exceeds configured minimum;
4. volume acceleration exceeds configured threshold;
5. market cap is inside configured range;
6. token/pool passes deterministic safety checks;
7. executable buy route exists;
8. executable reverse sell route exists;
9. expected execution economics are acceptable;
10. market has not already been consumed by the current trade instance.

Example configurable condition:

```text
volume_5m > $10,000
```

This is an initial example, not a permanently hard-coded strategy constant.

The implementation must place strategy parameters in configuration.

---

# 12. Safety Engine

The safety engine must be deterministic.

Potential checks:

- mint authority;
- freeze authority;
- Token-2022 extensions;
- supported token program;
- pool identity;
- pool liquidity;
- liquidity state;
- buy route;
- reverse sell route;
- expected output;
- price impact;
- minimum received;
- transaction simulation where practical;
- suspicious state transitions;
- unsupported program behavior.

The principle is:

> **Unknown is not safe.**

If Neurone cannot deterministically establish that the required execution path is valid, the candidate should be rejected or remain unarmed.

The safety engine must not claim to predict every rug.

It only establishes whether the known deterministic conditions are acceptable.

---

# 13. Pre-Arming

Pre-arming is one of Neurone's defining concepts.

Once a market qualifies, Neurone should prepare the trade before the trigger.

Conceptually:

```text
QUALIFICATION
     │
     ▼
calculate entry condition
calculate exit conditions
validate route
prepare accounts
prepare instructions
prepare compute budget
prepare priority parameters
prepare Beam submission context
     │
     ▼
ARMED
     │
     │   wait
     │
     ▼
TRIGGER
     │
     ▼
minimal finalization
     │
     ▼
SUBMIT
```

The system must not assume that every transaction field can remain valid indefinitely.

For example:

- recent blockhash;
- rapidly changing reserves;
- account balances;
- dynamic priority conditions;
- transaction simulation assumptions

may require finalization close to submission.

Therefore:

**Pre-arm everything that can safely be pre-armed.**

**Finalize only what must be fresh.**

---

# 14. Entry Trigger

The trigger must be deterministic.

Examples may include:

- price crossing a predefined threshold;
- executable pool state reaching a threshold;
- volume acceleration crossing a threshold;
- liquidity/price combination satisfying the armed condition.

The exact V1 trigger must be chosen before implementation.

Important:

The trigger must be based on **executable market state**, not merely a chart display.

---

# 15. Capital Engine

V1 assumes:

```text
Total working funds: $25 equivalent
Trading capital:     $20 equivalent
Fee reserve:          $5 SOL reserve
```

Only one $20 position may be active at a time in V1.

This creates a **capital bottleneck**, not a processing bottleneck.

While one position is active:

```text
Market A → observing
Market B → observing
Market C → armed
Market D → observing
Market E → qualified
...
```

All can continue processing.

The capital engine determines which armed opportunity is allowed to consume the single capital slot.

---

# 16. Capital Arbitration

When several armed markets trigger close together:

```text
Trigger A ─┐
Trigger B ─┼──► Capital Arbiter ──► one winner
Trigger C ─┘
```

The arbitration policy must be deterministic.

Possible initial rule:

1. earliest valid trigger;
2. if same slot, lowest measured trigger latency / deterministic tie-break;
3. reject or defer other candidates.

The first version should not attempt complicated opportunity ranking.

If the capital slot is occupied, Neurone should not queue stale trades indefinitely.

A trigger that has materially decayed should be discarded.

---

# 17. Position Engine

Once an entry lands, the position engine owns the position.

It records:

```text
entry quantity
entry executable value
entry transaction
entry slot
entry timestamp
entry execution price
fees
priority cost
actual received quantity
```

The position engine then monitors the live state in parallel with the rest of the system.

---

# 18. Exit Strategy

Initial target example:

```text
Entry capital: $20
TP:             $30 gross position value
SL:             $19.50 gross position value
```

The actual implementation must account for:

- swap fees;
- price impact;
- slippage;
- priority fee;
- Beam/Jito-related costs where applicable;
- actual token quantity;
- actual executable reverse route.

The system should calculate:

```text
realized_pnl =
    exit_value
  - entry_cost
  - trading_fees
  - priority_cost
  - other_execution_costs
```

The system should never claim a successful 1.5x trade merely because a chart showed 1.5x.

The relevant quantity is:

> **what Neurone could actually receive by executing the exit.**

---

# 19. Exit Hot Path

Once a position is open, TP and SL conditions should already be armed.

Conceptually:

```text
LIVE POSITION
     │
     ├── executable value >= TP ──► SELL
     │
     └── executable value <= SL ──► SELL
```

No LLM.

No strategy reevaluation.

No HTTP request to an external chart service.

No database query.

No narrative analysis.

Just:

```text
event
→ state update
→ threshold comparison
→ transaction finalization
→ Beam
```

---

# 20. Transaction Construction

The transaction subsystem must be optimized for predictable execution.

It should handle:

- account metas;
- instructions;
- compute budget;
- priority fee;
- required recent blockhash;
- signing;
- serialization;
- Beam submission.

Where possible, transaction components should be constructed before the trigger.

Where freshness is required, only the final dynamic values should be inserted at trigger time.

---

# 21. Execution Path

The desired entry path is:

```text
TRIGGER EVENT
     ↓
STATE ALREADY KNOWN
     ↓
ARMED TRANSACTION CONTEXT
     ↓
FINALIZE FRESH FIELDS
     ↓
SIGN
     ↓
BEAM
     ↓
LEADER
     ↓
LANDED
```

The exit path follows the same philosophy.

The system should measure every stage.

---

# 22. Latency Instrumentation

Latency measurement is a first-class feature.

At minimum record:

```text
chain_event_time
state_update_time
qualification_time
arming_time
trigger_time
transaction_finalize_time
submission_time
beam_ack_time
landed_time
exit_trigger_time
exit_submission_time
exit_landed_time
```

Derived metrics:

```text
event → state
state → qualification
qualification → armed
trigger → finalize
finalize → submit
submit → landed
trigger → landed
entry → exit
total trade duration
```

The most important metric is:

> **condition became true → transaction landed**

This is the real hot-path latency.

---

# 23. Hot Path vs Cold Path

Neurone must explicitly separate hot and cold work.

## Hot path

Allowed:

- Yellowstone event handling;
- compact state updates;
- deterministic calculations;
- threshold comparisons;
- precomputed transaction data;
- signing;
- Beam submission.

Not allowed:

- LLM calls;
- HTTP APIs;
- heavy serialization;
- database writes;
- logging huge payloads;
- filesystem access;
- unnecessary allocations;
- slow external lookups.

## Cold/asynchronous path

Allowed:

- telemetry;
- persistent trade records;
- analytics;
- metrics;
- debugging;
- historical storage;
- detailed logs;
- future frontend feeds.

Cold-path failures must not stop trading.

---

# 24. Memory / Persistence

V1 does not require a heavyweight database in the execution path.

Runtime state lives in memory.

Async telemetry can persist:

```text
trades
market candidates
armed opportunities
execution results
latency measurements
errors
system health
```

The persistence mechanism can be chosen during implementation based on actual requirements.

The critical rule is:

> Persistence must never block the trading path.

---

# 25. Failure Handling

Neurone must fail closed.

Examples:

### Yellowstone disconnect

- stop new trading;
- preserve known state where possible;
- reconnect;
- rehydrate required state;
- resume only after stream integrity is restored.

### Beam unavailable

- stop new submissions;
- do not pretend an order was sent;
- preserve state;
- retry according to deterministic policy.

### Stale state

- invalidate affected armed trades;
- do not execute from stale assumptions.

### Transaction construction failure

- abort that trade;
- record reason;
- release capital state correctly.

### Entry submitted but landing uncertain

- reconcile before submitting a duplicate;
- prevent double-spend/double-entry.

### Exit submission uncertain

- position remains logically open until reconciled;
- do not assume it is closed.

### Unexpected program behavior

- reject/disable affected market;
- never attempt speculative execution.

---

# 26. Duplicate Protection

Every execution must have deterministic identity.

Potential identity:

```text
strategy_id
+ mint
+ pool
+ qualification_epoch
+ trigger_slot
```

The execution engine must prevent:

- duplicate entry;
- duplicate exit;
- stale trigger execution;
- replayed Yellowstone event causing duplicate submission.

---

# 27. Event Ordering

Solana is slot-driven, and state updates can arrive through different update types.

Neurone must distinguish:

- event arrival time;
- slot;
- block/transaction ordering;
- local processing time.

The system must not confuse:

```text
"I received this event first"
```

with:

```text
"This happened first on-chain"
```

Per-pool ordering should be preserved where strategy correctness requires it.

---

# 28. Configuration

Strategy and runtime parameters must be configurable.

Example:

```toml
[strategy]
min_liquidity = ...
min_volume_5m = 10000
min_volume_acceleration = ...
min_market_cap = ...
max_market_cap = ...
take_profit_multiple = 1.5
max_loss = 0.50

[capital]
position_size = 20
fee_reserve = 5
max_open_positions = 1

[execution]
max_slippage = ...
priority_fee_policy = ...
beam_timeout_ms = ...

[runtime]
shards = ...
yellowstone_endpoint = ...
```

Exact values must not be invented during implementation.

They should be finalized from the strategy specification and measured execution conditions.

Secrets must never be committed.

---

# 29. Testing Strategy

Testing should mirror the architecture.

## Unit tests

- volume calculations;
- acceleration calculations;
- liquidity calculations;
- market-cap calculations;
- safety checks;
- state transitions;
- TP/SL calculations;
- P&L;
- capital arbitration;
- duplicate protection.

## Event replay tests

Feed recorded Yellowstone events into the state engine.

Verify:

```text
same input events
→ same state
→ same qualification
→ same arming
→ same trigger
```

The strategy should be deterministic.

## Transaction tests

Test:

- transaction construction;
- signing;
- account ordering;
- compute budget;
- priority configuration;
- serialization.

## Failure tests

Simulate:

- dropped stream;
- duplicate events;
- stale state;
- malformed updates;
- Beam failure;
- ambiguous landing;
- entry success + exit failure;
- process restart.

## Performance tests

Measure:

- events/sec;
- state updates/sec;
- allocations;
- lock contention;
- CPU;
- memory;
- trigger-to-submit;
- submit-to-land.

---

# 30. Observability

Neurone should expose internal metrics for future frontend use.

Core metrics:

```text
yellowstone_connected
events_per_second
active_markets
qualified_markets
armed_markets
rejected_markets
open_position
capital_available
entry_attempts
entry_successes
entry_failures
exit_attempts
exit_successes
exit_failures
realized_pnl
win_rate
average_trade_duration
average_trigger_to_land
p50_trigger_to_land
p95_trigger_to_land
p99_trigger_to_land
beam_status
```

The frontend is not part of this blueprint's implementation scope, but the backend must be designed so these metrics can later be exposed cleanly.

---

# 31. Frontend Boundary

Frontend design is deliberately deferred.

However, Neurone must expose a clean internal telemetry boundary so the future frontend does not require direct access to the trading internals.

Future frontend may eventually show:

- live system status;
- active markets;
- armed trades;
- current position;
- execution timeline;
- P&L;
- win/loss statistics;
- latency;
- Beam/Yellowstone health;
- recent trades;
- rejected candidates;
- strategy configuration.

The frontend must be an observer/control surface, not part of the hot execution path.

---

# 32. Security

Never expose:

- private keys;
- seed phrases;
- signing material;
- Beam credentials;
- Yellowstone credentials;
- API secrets.

Secrets must be supplied through environment/configuration mechanisms appropriate to deployment.

The frontend must never directly receive signing secrets.

Signing should remain server-side.

---

# 33. Repository Structure

The initial repository should remain simple.

Suggested structure:

```text
neurone/
├── README.md
├── BLUEPRINT.md
├── Cargo.toml
├── Cargo.lock
├── .env.example
├── config/
│   └── default.toml
├── src/
│   ├── main.rs
│   ├── config.rs
│   ├── ingestion/
│   ├── events/
│   ├── market/
│   ├── strategy/
│   ├── safety/
│   ├── arming/
│   ├── capital/
│   ├── execution/
│   ├── position/
│   ├── telemetry/
│   └── runtime/
├── tests/
│   ├── unit/
│   ├── replay/
│   └── integration/
└── fixtures/
```

The coding agent may refine the exact module layout if the resulting architecture remains faithful to this blueprint.

Do not create needless abstraction layers.

---

# 34. Neo Agent Skills

The coding agent is expected to use the existing **Neo Agent skills already installed on the VPS**.

Those skills are part of the development environment, not part of Neurone's runtime architecture.

The coding agent should:

1. inspect the existing Neo skills before implementation;
2. use the applicable Rust/build/test/debug skills;
3. follow the existing Neo agent workflow;
4. keep this blueprint as the project source of truth;
5. avoid inventing a competing agent architecture inside Neurone;
6. use the skills to validate implementation incrementally.

Neurone itself must not depend on the Neo skills at runtime.

---

# 35. Implementation Rules for the Coding Agent

Before writing substantial code, the agent must:

1. read this blueprint completely;
2. inspect the repository;
3. inspect the available Neo Agent skills;
4. inspect relevant Solami documentation/API references;
5. identify exact Yellowstone message/program requirements;
6. identify the exact transaction path required for Beam;
7. produce an implementation plan;
8. identify unresolved protocol assumptions;
9. avoid silently inventing missing Solana program details.

The agent must not rewrite the architecture because an easier implementation is available.

If a design change is necessary, it must be surfaced explicitly.

---

# 36. Build Order

Implementation should proceed in controlled increments.

## Stage 1 — Foundation

- Rust project;
- configuration;
- logging;
- error model;
- runtime skeleton;
- tests.

## Stage 2 — Yellowstone

- connection;
- authentication;
- subscriptions;
- reconnect;
- event normalization.

## Stage 3 — Parallel State

- sharding;
- market state;
- event routing;
- deterministic state updates.

## Stage 4 — Strategy

- filters;
- safety engine;
- qualification;
- rejection;
- deterministic state machine.

## Stage 5 — Pre-Arming

- armed trade structure;
- executable thresholds;
- transaction preparation;
- invalidation/expiry.

## Stage 6 — Capital + Execution

- capital arbiter;
- transaction finalization;
- signing;
- Beam submission;
- landing reconciliation.

## Stage 7 — Position + Exit

- live position state;
- executable TP/SL;
- exit construction;
- Beam submission;
- reconciliation.

## Stage 8 — Telemetry

- latency metrics;
- trade records;
- performance statistics;
- health metrics.

## Stage 9 — Replay/Stress

- event replay;
- deterministic verification;
- concurrency testing;
- performance profiling;
- failure injection.

## Stage 10 — Live Validation

Start with observation/paper execution.

Then:

```text
observe
→ qualify
→ arm
→ simulate
→ measure
→ tiny live execution
→ validate
→ scale only after evidence
```

---

# 37. Performance Philosophy

Do not optimize blindly.

First make the system correct.

Then measure.

Then optimize the actual bottleneck.

The priority is:

```text
correctness
→ deterministic behavior
→ instrumentation
→ profiling
→ hot-path optimization
```

Potential optimization targets:

- protobuf decoding;
- allocations;
- hash-map lookups;
- lock contention;
- shard routing;
- transaction construction;
- signing;
- serialization;
- Beam submission;
- network placement.

Rust should be used because deterministic low-overhead concurrent processing is a core requirement, not simply because Rust is fast.

---

# 38. Core Invariants

These invariants must remain true throughout development.

### Invariant 1

**Neurone is parallel.**

It must never become a serial token scanner.

### Invariant 2

**Market state is continuously maintained.**

Neurone does not repeatedly reconstruct the entire market from scratch.

### Invariant 3

**Trading decisions are deterministic.**

The same state produces the same decision.

### Invariant 4

**The trigger path is minimal.**

The system decides before the trigger whenever possible.

### Invariant 5

**Executable value matters more than chart value.**

TP/SL and P&L use executable economics.

### Invariant 6

**Capital limitation does not dictate market-processing architecture.**

One active position does not mean one market at a time.

### Invariant 7

**Unknown execution conditions are unsafe.**

Unverifiable state is rejected or remains unarmed.

### Invariant 8

**The hot path must not depend on slow external services.**

### Invariant 9

**Telemetry never blocks execution.**

### Invariant 10

**Frontend is outside the trading hot path.**

---

# 39. Latency Budget Concept

Neurone should ultimately think in terms of:

```text
event becomes true
        ↓
state update
        ↓
trigger comparison
        ↓
transaction finalization
        ↓
sign
        ↓
Beam
        ↓
leader
        ↓
land
```

The goal is not simply:

> "Detect memes quickly."

The stronger objective is:

> **Minimize the amount of work remaining after the trade condition becomes true.**

This is the central latency philosophy of Neurone.

---

# 40. V1 Definition of Done

Neurone V1 is complete when it can:

1. connect to Yellowstone;
2. maintain relevant market state concurrently;
3. process many markets independently;
4. apply deterministic filters;
5. perform deterministic safety/executability checks;
6. create armed opportunities;
7. wait for a deterministic trigger;
8. arbitrate the single capital slot;
9. construct/sign the transaction;
10. submit through Beam;
11. reconcile landing;
12. maintain the position;
13. deterministically trigger TP/SL;
14. execute the exit;
15. reconcile the exit;
16. calculate realized P&L;
17. record complete execution latency;
18. survive expected stream/execution failures;
19. replay events deterministically in tests;
20. demonstrate that the market-processing engine remains parallel under load.

A frontend is **not** required for V1 completion.

---

# 41. Final Architecture

The intended V1 architecture is:

```text
                         SOLANA
                            │
                            ▼
                  ┌──────────────────┐
                  │ SOLAMI YELLOWSTONE│
                  └─────────┬────────┘
                            │
                            ▼
                  ┌──────────────────┐
                  │ EVENT NORMALIZER │
                  └─────────┬────────┘
                            │
                    hash(pool/mint)
                            │
          ┌─────────────────┼─────────────────┐
          ▼                 ▼                 ▼
     ┌─────────┐       ┌─────────┐       ┌─────────┐
     │ SHARD 0 │       │ SHARD 1 │  ...  │ SHARD N │
     ├─────────┤       ├─────────┤       ├─────────┤
     │ Market A│       │ Market D│       │ Market X│
     │ Market B│       │ Market E│       │ Market Y│
     │ Market C│       │ Market F│       │ Market Z│
     └────┬────┘       └────┬────┘       └────┬────┘
          │                 │                 │
          └─────────────────┼─────────────────┘
                            ▼
                  ┌──────────────────┐
                  │ FILTER + SAFETY  │
                  └─────────┬────────┘
                            │
                            ▼
                  ┌──────────────────┐
                  │  PRE-ARM ENGINE  │
                  └─────────┬────────┘
                            │
                   thousands of armed/
                   observing states
                            │
                            ▼
                  ┌──────────────────┐
                  │ CAPITAL ARBITER  │
                  └─────────┬────────┘
                            │
                      one $20 slot
                            │
                            ▼
                  ┌──────────────────┐
                  │ EXECUTION ENGINE │
                  └─────────┬────────┘
                            │
                            ▼
                     SOLAMI BEAM
                            │
                            ▼
                         SOLANA
                            │
                            ▼
                  ┌──────────────────┐
                  │ POSITION ENGINE  │
                  └─────────┬────────┘
                            │
                       TP / SL
                            │
                            ▼
                     SOLAMI BEAM
                            │
                            ▼
                         SOLANA


          ┌─────────────────────────────────────┐
          │ ASYNC TELEMETRY / OBSERVABILITY    │
          │                                     │
          │ trades • P&L • latency • health    │
          │ events • errors • execution stats  │
          └─────────────────────────────────────┘
```

---

# 42. Architectural Summary

Neurone is not:

```text
scan → analyze token 1 → trade → scan token 2 → ...
```

It is:

```text
Yellowstone
    ↓
continuous event stream
    ↓
parallel state machines
    ↓
deterministic qualification
    ↓
pre-armed opportunities
    ↓
trigger
    ↓
capital arbitration
    ↓
Beam
    ↓
position
    ↓
armed exit
    ↓
Beam
```

The system's defining properties are:

**parallelism, determinism, pre-arming, executable-state awareness, and measured execution latency.**

The frontend should later be designed around the state and telemetry produced by this architecture, rather than changing the architecture to suit a UI.
