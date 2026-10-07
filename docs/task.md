# NEURONE — POST-M1/M2/M3 REQUIRED SAFETY FIXES

## Role

You are DeepSeek operating on the Neurone repository at:

`/home/xion/neurone`

Use the available Neo Agent skills on the VPS before and during the work.

This is an implementation task based on the completed read-only M1/M2/M3 codebase audit:

`M1_M2_M3_CODEBASE_AUDIT.md`

The audit is the basis for the fixes below.

---

## 1. Objective

Implement **only the required fixes identified by the audit**.

Do NOT redesign Neurone.
Do NOT reopen M1/M2/M3.
Do NOT begin M4.
Do NOT add ARM, trigger, strategy, capital, execution, TP/SL, or trading behavior.

The user's instruction is:

> Make the required fixes. Don't act where it isn't needed.

Therefore this task is deliberately narrow.

The two fixes that should actually be implemented now are:

1. **Fix failed-transaction state mutation.**
2. **Fix M3.3 invalidation recovery under the shipped/default runtime configuration.**

The audit explicitly says the pre-state corroboration gate and quote wiring are M4 work and should **not** be implemented in this task.

---

# 2. Source of Truth

Read completely before editing:

- `NEURONE_BLUEPRINT.md`
- `M1_M2_M3_CODEBASE_AUDIT.md`

Also inspect the relevant current source and existing tests.

Trust current source code and tests over historical milestone reports.

Do not make changes merely because an old report says something should exist.

---

# 3. Fix A — Failed Transactions Must Not Mutate Market State

## Problem

The audit found:

- `TransactionUpdate.success` is computed.
- The runtime currently does not consume it before applying decoded swaps/creates.
- A failed/reverted transaction carrying a decoded program event can therefore mutate `MarketState`.
- Reserve-mutation invalidation can also currently react to failed transactions.

This is a real correctness/safety defect.

## Required behavior

For a transaction where:

`TransactionUpdate.success == false`

the runtime must **not apply economic state mutations from that transaction**.

At minimum, failed transactions must not:

- apply decoded swaps;
- apply decoded creates;
- alter market reserves/volume/trade state;
- cause reserve-mutation invalidation.

Do not invent additional semantics beyond what is necessary.

Keep normalization and observability intact where useful; this fix concerns whether failed transaction contents are allowed to mutate authoritative market state.

## Implementation requirements

- Locate the existing transaction application path in the shard/runtime.
- Add the smallest correct guard at the appropriate boundary.
- Prefer one centralized guard over scattered checks if that is cleaner and safer.
- Preserve successful transaction behavior exactly.
- Preserve existing Yellowstone ingestion behavior.
- Preserve telemetry unless a specific metric would become misleading.
- Do not change protocol decoding formulas.
- Do not change quote math.
- Do not change shard architecture.

## Tests

Add focused deterministic regression coverage proving:

1. A successful transaction still applies its decoded swap/create.
2. A failed transaction does not apply its decoded swap.
3. A failed transaction does not create a market from a decoded create.
4. A failed reserve-mutation transaction does not invalidate market state.

Use the existing test style and helpers.

Do not create broad or redundant tests.

---

# 4. Fix B — Make M3.3 Invalidation Recoverable in the Shipped Runtime

## Problem

The audit found:

- `MarketState::invalidate()` correctly marks a market `Invalidated`.
- `apply_account()` can clear invalidation when a fresh authoritative account update arrives.
- `apply_swap()` currently does not clear invalidation.
- The shipped/default configuration has:

  `account_programs = []`

  `account_addresses = []`

- Therefore the default `neurone run` path does not receive the bonding-curve account updates required to recover an invalidated pump.fun market.
- A market can therefore remain permanently invalidated.

This is a real availability defect in the shipped configuration.

## Required behavior

After a reserve mutation invalidates a market, the market must remain fail-closed until a **fresh authoritative state** proves that the market state is usable again.

Do NOT simply clear invalidation on any arbitrary event.

The recovery rule must be deterministic and conservative.

The audit explicitly identified the candidate direction:

- a fresh authoritative pump.fun bonding-curve account update, OR
- a post-mutation authoritative `TradeEvent`/state transition when that event itself provides sufficient proof of current reserves.

You must inspect the current code and existing state model before choosing the minimal implementation.

### Important safety constraint

Do NOT turn this into:

`INVALIDATED -> any swap -> KNOWN`

unless the swap provides sufficient authoritative reserve information to prove the state is current and coherent.

Do not weaken fail-closed behavior merely to make the market resume.

If a safe recovery mechanism cannot be established from existing event/account information, keep the market invalidated and report that limitation rather than inventing a heuristic.

## Default-runtime requirement

The fix must work under the actual shipped/default runtime configuration.

Do not solve the problem only by adding a test that manually injects an account update while the production configuration still receives no account updates.

If subscribing to bonding-curve accounts is the safest minimal solution, assess its impact on the existing Yellowstone subscription architecture and configuration before changing it.

Do not add polling/RPC.

Do not introduce a new external service.

Do not redesign the ingestion architecture.

## Ordering requirement

The audit found that the code stores `TransactionUpdate.index` but does not currently use it.

Do NOT expand this task into implementing transaction-index ordering unless it is strictly necessary to make the invalidation recovery fix correct.

Do not change unrelated ordering behavior.

The goal is the smallest correct fix.

---

# 5. Tests for Fix B

Add focused deterministic tests covering the actual recovery rule you implement.

At minimum:

1. Market becomes `Invalidated` after a reserve mutation.
2. An older/non-authoritative update does NOT recover it.
3. A fresh authoritative state update DOES recover it.
4. Once recovered, the reserve state becomes usable again only when the existing freshness/validity rules permit it.
5. If recovery proof is insufficient, the state remains `Invalidated`.

Also add a regression test that exercises the **runtime/default configuration path** relevant to the defect, if practical within the existing architecture.

Do not fake a production success merely by directly injecting an account event if the shipped runtime would never receive that event.

---

# 6. Do NOT Implement These Items

Explicitly leave these alone:

### M4 pre-state corroboration

The audit says the `validate.rs` corroboration gate is not in the runtime.

That is an M4 requirement.

**Do not move or rewrite it now.**

### Quote engine production wiring

`quote()` is not currently on the production runtime path.

That is expected before M4.

**Do not wire quote → strategy → arm now.**

### ARM / trigger / strategy

Do not implement:

- ARM
- trigger detection
- strategy scoring
- market selection
- capital arbiter
- execution
- Beam
- entry
- exit
- TP/SL

None of these belong in this task.

### Transaction index ordering

The audit notes the M3.3 report overclaimed this.

Do not implement transaction-index ordering unless the minimal safe recovery fix genuinely requires it.

### Protocol formula changes

Do not alter:

- Pump.fun formulas
- PumpSwap formulas
- fee primitives
- quote math
- decoder layouts

### Architecture changes

Do not:

- add global locks;
- add DB/RPC/HTTP to the hot path;
- replace Yellowstone;
- replace shard ownership;
- redesign the engine;
- introduce polling.

---

# 7. Required Validation

After implementation:

Run the appropriate existing checks, including where practical:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --locked
cargo build --release --locked
```

Run focused tests for the changed behavior.

If live Solami validation is safe and credentials are already available, you may run the existing read-only live test/validation relevant to the changed behavior.

Do not fabricate live results.

Do not make unrelated source changes just to satisfy a test.

---

# 8. Regression Requirements

Before finishing, verify that:

- Existing M1 tests remain passing.
- Existing M2 decoder/state/quote tests remain passing.
- Existing M3 parity tests remain passing.
- Existing mutation tests remain passing.
- Successful transaction behavior is unchanged.
- Supported quote parity remains exact.
- No new warnings.
- Formatting is clean.
- No new global locks or blocking I/O were introduced.
- No polling/RPC/HTTP was introduced into the hot path.

Pay particular attention to proving that the fixes do not regress the independently verified M3 behavior.

---

# 9. Scope Discipline

This is a **small corrective implementation**, not a new milestone.

Before editing each file, ask:

> Is this file actually required for one of the two fixes?

If not, leave it unchanged.

Avoid:

- refactoring unrelated code;
- renaming unrelated APIs;
- cleanup unrelated to these defects;
- rewriting reports;
- changing blueprint files;
- speculative hardening;
- adding abstractions for future M4 work.

Minimal diff is preferred.

---

# 10. Required Deliverable

Create:

`M1_M2_M3_REQUIRED_FIXES_REPORT.md`

The report must contain:

## A. Fixes implemented

For each of the two fixes:

- problem;
- exact implementation;
- files changed;
- why the implementation is safe;
- why it is minimal.

## B. Recovery semantics

Clearly state the final state transition for mutation recovery, for example:

`KNOWN -> INVALIDATED -> [authoritative recovery condition] -> KNOWN`

Use the actual implemented condition, not a hypothetical one.

## C. Tests

List:

- focused tests added/changed;
- full test result;
- clippy;
- fmt;
- release build;
- live validation if actually run.

## D. Files changed

Give an exact list.

## E. Files intentionally NOT changed

Mention the major items deliberately left alone:

- M4 corroboration gate;
- quote production wiring;
- ARM/strategy/execution;
- transaction-index ordering unless actually required;
- protocol math;
- architecture.

## F. Remaining limitations

Only list limitations that genuinely remain after these fixes.

Do not invent new work.

## G. Final verdict

Use one of:

- `FIXES COMPLETE — READY FOR M4`
- `FIXES COMPLETE — READY FOR M4 WITH LIMITATIONS`
- `NOT COMPLETE`

The verdict must be based on actual tests and source inspection.

---

# 11. Final Rule

The objective is **not** to make the code "more complete."

The objective is to correct the two substantiated defects from the M1/M2/M3 audit and then stop.

If something is not required for:

1. failed-transaction safety, or
2. safe M3.3 invalidation recovery,

**do not change it.**

Do not implement M4.
Do not anticipate M4.
Do not broaden the scope.

At the end, report exactly what changed and what was intentionally left untouched.
