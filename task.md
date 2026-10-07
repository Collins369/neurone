# Neurone — Milestone 1 Implementation Task

You are the coding agent implementing **Neurone**, a standalone Solana trading system.

## SOURCE OF TRUTH

`NEURONE_BLUEPRINT.md` is the authoritative architecture.

Read it completely before writing substantial code.

Neurone is completely separate from Xion. Do not import Xion's six-layer architecture or terminology.

The existing **Neo Agent skills on this VPS are already available to you**. Inspect them first and use the relevant skills for Rust development, repository workflow, testing, debugging, and validation.

You are running under **DeepSeek in Codex**. Work methodically, inspect before changing, and do not invent undocumented APIs or project requirements.

---

# CURRENT MISSION

Implement **Milestone 1 only**:

> Build the Neurone runtime foundation + live Solami Yellowstone ingestion + parallel sharded market-state engine.

Do NOT implement live trading yet.

The milestone should establish:

```text
Solami Yellowstone
        ↓
Rust gRPC ingestion
        ↓
event normalization
        ↓
parallel sharded market-state engine
        ↓
telemetry
```

The objective is to prove that Neurone's core architecture is genuinely **parallel and event-driven**, not a serial token scanner.

---

# 1. BEFORE CODING

Perform these steps first:

1. Read `NEURONE_BLUEPRINT.md` completely.
2. Inspect the repository/workspace.
3. Inspect the installed Neo Agent skills and identify the relevant skills.
4. Establish the current official Solami documentation before implementing any Solami integration:
   - First fetch and read `https://solami.dev/llms.txt`. Solami explicitly provides this endpoint as the structured documentation index for AI agents.
   - From that index, follow the relevant official Yellowstone gRPC documentation, API reference, Rust/client examples, authentication requirements, subscription/filter documentation, and any relevant snippets.
   - Do not rely on memory, stale examples, third-party tutorials, or guessed APIs when the official documentation can establish the correct interface.
   - Record the exact Solami endpoint/authentication/subscription details actually used by the implementation.
5. Inspect any existing environment/configuration relevant to Solami.
6. Determine the correct project/repository location.
7. Identify any missing information that cannot safely be inferred.

Do not silently invent undocumented Solami interfaces.

If something genuinely cannot be established from the available documentation/environment, document the assumption instead of guessing.

## Solami documentation research contract

Before writing the Yellowstone client, verify and record at minimum:

- Yellowstone gRPC endpoint and connection requirements;
- authentication/API-key requirements and where credentials belong;
- supported Rust/client implementation path;
- `SubscribeRequest` shape and required fields;
- account/program/transaction/slot filters relevant to Neurone;
- commitment/finality behavior;
- keepalive/ping requirements;
- `SubscribeUpdate` event types and the fields Neurone actually needs;
- reconnect/error semantics;
- any documented limits, quotas, or PAYG constraints that affect the implementation.

Use the official Solami documentation as the implementation authority. If the docs have changed since this task was written, implement against the current documented API rather than this task's wording.

The research must happen **before** the Yellowstone integration is coded. Do not write a guessed client and retrofit the docs afterward.

---

# 2. RUNTIME FOUNDATION

Create/establish a clean Rust project.

Include:

- Cargo configuration;
- application entry point;
- configuration handling;
- structured logging;
- deterministic error handling;
- graceful shutdown;
- runtime/task structure;
- environment-based secrets.

Secrets must never be committed.

Do not introduce unnecessary frameworks or abstraction layers.

Keep the project small and understandable.

---

# 3. SOLAMI YELLOWSTONE

Implement the live Yellowstone ingestion layer.

It must support:

- authenticated connection;
- persistent gRPC stream;
- required subscription configuration;
- stream health monitoring;
- reconnect handling;
- graceful shutdown;
- slot/block metadata;
- event arrival instrumentation.

Use the actual Solami interface discovered from the current documentation/configuration.

Do not fabricate method names, endpoints, message formats, or authentication behavior.

The Yellowstone layer should be responsible for receiving chain data, not trading decisions.

---

# 4. EVENT NORMALIZATION

Create a compact internal event representation.

The system should convert relevant Yellowstone protobuf/update messages into internal Neurone events.

Potential event categories include:

- slot updates;
- transaction updates;
- account updates;
- pool-related updates;
- token-related updates;
- swap-related updates;
- liquidity/state changes.

Only implement event types that are actually required/available for this milestone.

Do not prematurely implement the entire Solana event universe.

Avoid carrying unnecessarily large raw protobuf objects through every processing stage.

The normalized event should retain the information needed for later deterministic market-state reconstruction.

---

# 5. PARALLEL MARKET STATE

Implement the core parallel state engine.

The architecture should conceptually be:

```text
Yellowstone event
       ↓
normalize
       ↓
hash(pool/mint)
       ↓
shard
       ↓
market state
```

Create multiple independent state shards.

Each shard owns the market states assigned to it.

Requirements:

- deterministic shard routing;
- consistent pool/mint → shard mapping;
- per-market state ownership;
- preservation of ordering where required;
- no single global lock protecting all market state;
- independent markets must be capable of processing concurrently.

The architecture must demonstrate **many independent state machines**, not one global event loop doing all market work sequentially.

---

# 6. MARKET STATE

Create only the minimum state necessary for this milestone.

It should be possible to evolve toward fields such as:

```text
mint
pool
last_slot
last_update
liquidity
price
volume/state fields where actually available
market status
```

Do NOT prematurely implement the complete strategy.

Strategy fields such as:

- 5m volume threshold;
- volume acceleration;
- market-cap filter;
- TP;
- SL;

belong to later milestones unless a field is naturally required to validate the state engine.

---

# 7. TELEMETRY

Implement basic operational telemetry.

At minimum measure:

```text
yellowstone_connected
events_received
events_per_second
slots_observed
event_processing_latency
active_market_states
shard_activity
reconnect_count
invalid_events
errors
```

Also make it possible to later measure:

```text
event timestamp
→ state update timestamp
→ trigger timestamp
→ submission timestamp
→ landing timestamp
```

Do not build the future frontend.

Do not build a heavyweight analytics database.

Telemetry must not block event processing.

---

# 8. TESTING

Write tests for the actual architecture.

At minimum:

### Event normalization
Identical Yellowstone inputs should produce deterministic normalized events.

### State determinism
Identical event sequences should produce identical market state.

### Shard routing
The same market identity must always route to the same shard.

### Ordering
Events for the same market must preserve the ordering required by the state engine.

### Parallelism
Independent markets must be capable of processing concurrently.

The test should demonstrate that the implementation is not simply:

```text
for event:
    process everything synchronously
```

### Duplicate events
Repeated/replayed events must not corrupt state.

### Disconnect/reconnect
The ingestion layer must recover safely from a stream interruption.

### Shutdown
The runtime should terminate cleanly without corrupting state.

---

# 9. PERFORMANCE VALIDATION

Do not blindly micro-optimize.

First establish correctness.

Then measure:

- events/sec;
- state updates/sec;
- event-processing latency;
- allocation behavior;
- shard throughput;
- lock contention;
- CPU usage;
- memory usage.

If a bottleneck is found, identify it before changing architecture.

---

# 10. EXPLICITLY DO NOT IMPLEMENT

Do NOT implement any of the following in this milestone:

- live trading;
- transaction construction for actual trades;
- signing of live trades;
- Beam submission;
- TP/SL;
- capital arbitration;
- $20 position execution;
- strategy scoring;
- narrative analysis;
- LLM calls;
- DeepSeek calls from the Neurone runtime;
- Blur;
- ShredDirect;
- Index Engine;
- Webhooks;
- Mirage;
- frontend;
- heavyweight database;
- portfolio management;
- autonomous strategy modification.

DeepSeek is the coding/reasoning agent being used to build Neurone. It is NOT a runtime component of Neurone.

---

# 11. ARCHITECTURAL INVARIANTS

### Invariant 1 — Parallel

Neurone is a parallel event-driven state system.

It must NOT become:

```text
scan token
→ analyze
→ decide
→ trade
→ scan next token
```

It must behave like:

```text
                 Yellowstone
                      │
         ┌────────────┼────────────┐
         ▼            ▼            ▼
      Shard 0      Shard 1      Shard N
         │            │            │
      markets      markets      markets
         │            │            │
         └────────────┼────────────┘
                      ▼
               future strategy
```

### Invariant 2 — State is continuous

Do not repeatedly reconstruct the whole market from scratch for every decision.

Maintain state incrementally as events arrive.

### Invariant 3 — Deterministic

Same event sequence → same state.

### Invariant 4 — Minimal hot path

Do not place slow external services or unnecessary I/O in the event-processing path.

### Invariant 5 — No premature Solami products

Only use Solami components that are actually required.

For this milestone, Yellowstone is the central Solami component.

Beam belongs to the later execution milestone.

### Invariant 6 — Neo skills are development tooling

Use the existing Neo Agent skills.

Do not make Neurone runtime-dependent on them.

---

# 12. CODE QUALITY

Prefer:

- explicit data flow;
- small modules;
- clear ownership;
- predictable concurrency;
- minimal cloning;
- bounded channels where appropriate;
- clear error propagation;
- tests close to behavior;
- comments explaining architectural decisions rather than obvious syntax.

Avoid:

- speculative abstractions;
- unnecessary traits;
- premature generic frameworks;
- global mutable state;
- giant modules;
- hidden background behavior;
- blocking I/O in async hot paths;
- excessive logging of raw Yellowstone payloads.

---

# 13. DELIVERABLE

At the end of this task, the project must:

1. build successfully;
2. pass its tests;
3. connect to Solami Yellowstone using the real configured interface;
4. receive live events;
5. normalize relevant events;
6. route market events through deterministic shards;
7. maintain multiple market states concurrently;
8. expose useful runtime telemetry;
9. handle disconnect/reconnect safely;
10. shut down cleanly;
11. demonstrate the architecture is parallel rather than serial.

There must be **NO live trading**.

---

# 14. STOP CONDITION

After completing this milestone, STOP.

Do not continue into strategy/trading/Beam implementation.

Report:

1. files created/changed;
2. repository structure;
3. Neo Agent skills used;
4. Solami interfaces actually used;
5. Yellowstone subscription details;
6. event types implemented;
7. parallel-state architecture;
8. tests and results;
9. measured events/sec and latency if available;
10. reconnect behavior;
11. unresolved assumptions/issues;
12. recommended next milestone.

Do not claim a feature works unless it was actually tested.

---

# FINAL PRINCIPLE

Neurone's defining optimization is not merely:

> detect a token quickly.

It is:

> **make as much of the decision as possible before the trigger, so that when the trigger arrives, almost nothing remains between the chain event and transaction submission.**

This milestone builds the parallel nervous system that makes that possible.

Do not compromise that architecture for implementation convenience.
