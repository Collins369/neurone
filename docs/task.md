# NEURONE — M1/M2/M3 CODEBASE AUDIT

## Objective

Perform a read-only engineering audit of the CURRENT Neurone repository.

The purpose is to determine whether the code that is actually present in the repository genuinely satisfies the objectives of:

- M1 — Foundation / Yellowstone
- M2 — Real Market State
- M3 — Parallel State / Live State Correctness

Do NOT implement new features.
Do NOT fix code.
Do NOT create a new milestone.
Do NOT move into M4/M5.

This is an audit only.

The milestone reports may claim that something is complete, but the repository code is the authority for this audit. Verify every important claim against the actual implementation and tests.

---

# 1. Required workflow

Before reviewing:

1. Read `NEURONE_BLUEPRINT.md` completely.
2. Inspect the current repository structure.
3. Inspect the available Neo Agent skills on the VPS and use the relevant Rust/code-review/build/test skills.
4. Read the existing M1/M2/M3 milestone reports that are present in the repository.
5. Compare the reports against the actual source code.
6. Run appropriate existing tests/build/checks where practical.
7. Do NOT modify source code, configuration, tests, or reports.

If a command would modify the repository, do not run it.

The audit must be based primarily on:
- actual source code;
- actual tests;
- actual build/check results;
- blueprint requirements.

Use milestone reports as claims to verify, not as proof by themselves.

---

# 2. Audit M1 — Foundation / Yellowstone

Determine whether the current code genuinely satisfies the M1 objectives.

Review:

### Rust/runtime foundation

- Cargo project structure;
- runtime entry point;
- configuration;
- error model;
- logging/telemetry;
- runtime lifecycle;
- graceful failure behavior;
- test structure.

### Yellowstone

Inspect the actual implementation of:

- Solami Yellowstone connection;
- authentication;
- TLS;
- subscription construction;
- account filters;
- transaction filters;
- slot/block subscriptions;
- processed commitment;
- reconnect logic;
- backoff;
- stale-stream detection;
- replay/from-slot behavior if implemented;
- malformed/unexpected update handling.

### Normalization

Verify:

- raw Yellowstone messages are converted into compact internal events;
- relevant slot/transaction metadata is preserved;
- program attribution is deterministic;
- irrelevant events do not accidentally enter the trading/state path.

### Parallel routing

Verify:

- events are routed to shards;
- a market belongs to a deterministic shard;
- the design does not depend on one global market lock;
- routing does not secretly serialize all market processing.

### M1 tests

Run existing M1-related tests and identify:

- what is actually tested;
- what is only mocked;
- what is only unit-tested;
- what is genuinely live/integration tested;
- any missing critical coverage.

### M1 verdict

Give:

`PASS`, `PASS WITH LIMITATIONS`, `PARTIALLY RESOLVED`, or `FAIL`.

Do not give PASS merely because the milestone report says PASS.

---

# 3. Audit M2 — Real Market State

Determine whether the current implementation genuinely satisfies M2.

Review actual code for:

### Market decoding

- Pump.fun bonding curve decoding;
- PumpSwap/AMM decoding;
- account layouts;
- instruction/event layouts;
- discriminators;
- Borsh decoding;
- PDA/market identity logic;
- malformed/unknown layout handling.

### Market state

Verify whether `MarketState` actually maintains the relevant information required by the blueprint, including where applicable:

- mint;
- pool/market identity;
- reserves;
- liquidity;
- price;
- volume;
- slot;
- venue/protocol;
- lifecycle;
- executable state;
- freshness/state validity.

Do not assume a field is meaningful merely because it exists. Trace where it is populated and updated.

### State updates

Trace the complete path:

```text
Yellowstone
→ normalization
→ decoding
→ shard routing
→ MarketState update
```

Determine whether the state actually remains continuously maintained rather than repeatedly reconstructed.

### Quote/state relationship

Review quote calculations and determine:

- what state they consume;
- whether they use exact integer arithmetic;
- whether unsupported/stale/invalid state can accidentally be quoted;
- whether the quote implementation matches the intended protocol semantics;
- whether any known exclusions are safely rejected.

Do not redesign quote math during this audit.

### M2 tests

Run the relevant tests and verify:

- real fixtures;
- decoder tests;
- state tests;
- live tests if available;
- parity tests.

Separate:
- exact supported behavior;
- known exclusions;
- untested behavior;
- claims unsupported by the current code.

### M2 verdict

Give:

`PASS`, `PASS WITH LIMITATIONS`, `PARTIALLY RESOLVED`, or `FAIL`.

---

# 4. Audit M3 — Parallel State / Live State Correctness

This is the most important part of the audit.

Review the complete M3 implementation.

## A. Parallel state architecture

Verify:

- shard ownership;
- deterministic shard selection;
- per-market independence;
- event routing;
- absence of a global market-state lock;
- whether one busy market can stall unrelated markets;
- whether state updates are deterministic.

Do not confuse "async" with "parallel." Inspect the actual execution model.

Determine whether the architecture still matches:

> many independent state machines + minimal synchronization

from the blueprint.

---

## B. State ordering

Inspect how the code handles:

- slot;
- transaction index;
- instruction/event ordering where available;
- account write version;
- arrival order vs chain order;
- same-slot events;
- out-of-order account updates;
- duplicate updates.

Determine whether the ordering assumptions are actually supported by fields provided by Yellowstone.

Flag any invented or unjustified ordering assumptions.

---

## C. State freshness and validity

Inspect the actual implementation of:

- `UNKNOWN`;
- `KNOWN`;
- `STALE`;
- `INVALIDATED`;
- state version;
- invalidation slot;
- account refresh/revalidation;
- stale update rejection.

Verify the transition rules from code.

Particularly verify:

```text
KNOWN
  ↓
state-changing mutation
  ↓
INVALIDATED
  ↓
fresh authoritative update
  ↓
KNOWN
```

Do not test ARM/trigger/re-arm behavior. That belongs to the later Pre-Arming stage and is outside this audit.

---

## D. M3.2 validation gate

Review the actual pre-state validation logic.

Verify:

- previous-event handling;
- post-state anchoring;
- same-slot handling;
- exact state equality;
- account-cache corroboration;
- `UnsupportedState`;
- fail-closed behavior.

Determine whether the implementation can accidentally quote a state that has not been proven.

---

## E. M3.3 mutation handling

Inspect the actual implementation of the Pump.fun reserve mutation handling.

Verify:

- `SweepProtocolFee`;
- `SweepCreatorFee`;
- mutation detection;
- market invalidation;
- state-version increment;
- invalidation slot;
- fresh account recovery;
- stale account update rejection;
- quote refusal while invalidated;
- whether mutation affects only the appropriate market(s);
- whether unrelated markets continue processing.

Do not attempt to reverse-engineer or implement the sweep payload.

The purpose is to audit the current implementation.

---

## F. Fail-closed behavior

Look specifically for paths where the code might accidentally do:

```text
unknown state → assume state → quote → trade
```

or:

```text
stale state → continue using old quote
```

or:

```text
mutation observed → silently ignore
```

Any such path is a critical finding.

---

# 5. M3 performance / parallelism audit

Review existing benchmark/test evidence and, where practical, run safe read-only performance tests.

Look for:

- global mutexes;
- global RwLocks;
- synchronous blocking in the hot path;
- database calls;
- RPC calls;
- HTTP calls;
- filesystem I/O;
- unbounded allocations;
- unnecessary protobuf retention;
- serial iteration over all markets;
- mutation handling that scans unrelated markets.

Do not optimize anything.

Simply identify whether the implementation respects the blueprint's hot-path constraints.

---

# 6. Code quality / architecture audit

Inspect for:

- dead code;
- TODOs that affect milestone correctness;
- placeholders;
- fake/synthetic implementations accidentally used by production paths;
- duplicated logic;
- inconsistent state representations;
- unsafe assumptions;
- error paths that silently continue;
- configuration values that are ignored;
- tests that don't exercise production code;
- comments/docs that contradict implementation.

Pay particular attention to cases where a milestone report says something is implemented but the production path does not actually use it.

---

# 7. Blueprint compliance

Compare the current implementation against these core invariants from the blueprint:

1. Neurone is parallel.
2. Market state is continuously maintained.
3. Trading decisions are deterministic.
4. The future trigger path is minimal.
5. Executable value matters more than chart value.
6. Capital limitation does not dictate market-processing architecture.
7. Unknown execution conditions are unsafe.
8. Hot path does not depend on slow external services.
9. Telemetry does not block execution.
10. Frontend is outside the trading hot path.

For M1–M3, focus especially on invariants 1, 2, 3, 7, and 8.

Do not penalize the repository for components that belong to later milestones.

---

# 8. Run verification

Run appropriate non-mutating commands such as:

```text
cargo test
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
cargo build --release
```

If a command is too expensive or unavailable, state that explicitly.

If live tests require credentials and cannot run, do not fabricate results.

Record exact results.

---

# 9. Compare reports against reality

Create a table:

| Milestone | Report claim | Code evidence | Test evidence | Actual assessment |
|---|---|---|---|---|
| M1 | ... | ... | ... | PASS/PARTIAL/FAIL |
| M2 | ... | ... | ... | PASS/PARTIAL/FAIL |
| M3 | ... | ... | ... | PASS/PARTIAL/FAIL |

Identify:

### Overclaims

Things the reports claim are complete but the code does not fully demonstrate.

### Underclaims

Things the code actually handles better than the reports suggest.

### Important gaps

Things required by M1–M3 that are genuinely missing.

Do not invent gaps simply because a feature belongs to M4+.

---

# 10. No implementation

This is a STRICT READ-ONLY AUDIT.

Do NOT:

- modify Rust code;
- modify Cargo files;
- modify configuration;
- modify tests;
- modify reports;
- refactor;
- fix bugs;
- add tests;
- add dependencies;
- create M3.3A implementation;
- implement ARM;
- implement M4 strategy;
- implement Beam;
- implement transaction execution.

If you discover a problem, report it.

Do not fix it.

---

# 11. Deliverable

Create:

`M1_M2_M3_CODEBASE_AUDIT.md`

The report must contain:

## 1. Executive Summary

One concise overall assessment.

## 2. Repository / Architecture Reviewed

List the important modules inspected.

## 3. M1 Audit

Requirements, code evidence, tests, findings, verdict.

## 4. M2 Audit

Requirements, code evidence, tests, findings, verdict.

## 5. M3 Audit

Requirements, code evidence, tests, findings, verdict.

## 6. Cross-Milestone Findings

Important architectural issues spanning M1–M3.

## 7. Report-vs-Code Discrepancies

Explicitly identify anything where the reports overstate or understate reality.

## 8. Critical Findings

Rank:

- CRITICAL
- HIGH
- MEDIUM
- LOW
- INFORMATIONAL

Only use CRITICAL/HIGH when justified by actual code evidence.

## 9. Tests Actually Run

Exact commands and results.

## 10. Blueprint Compliance

Explicitly assess the relevant invariants.

## 11. Current Readiness

State clearly whether the codebase is ready to move to M4.

Use one of:

- `READY FOR M4`
- `READY FOR M4 WITH LIMITATIONS`
- `NOT READY FOR M4`

If not ready, identify the smallest concrete blocker(s).

## 12. Recommended Next Action

Do not create a new implementation plan.

Simply state the minimum next action based on the evidence.

---

# Final rule

The goal is NOT to make the milestone status look good.

The goal is to answer one question accurately:

> "If we ignored all previous milestone reports and inspected the current Neurone code today, would we honestly say M1, M2, and M3 objectives have been implemented correctly?"

Be skeptical.

Use source code and test evidence over documentation claims.

Do not invent certainty where the repository does not provide it.
