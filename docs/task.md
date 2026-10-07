# NEURONE — M2.1 CORRECTNESS HARDENING TASK

**Repository:** `/home/xion/neurone`  
**Source of truth:** `NEURONE_BLUEPRINT.md`  
**Scope:** M2.1 only — harden the two known M2 market-state/execution-pricing limitations before M3.  
**Agent:** DeepSeek via Codex  
**Development tooling:** Existing Neo Agent skills at `/home/xion/neo-agent`

## Mission

Harden the completed M2 implementation without expanding Neurone into a trading system.

M2 successfully live-verified pump.fun bonding curves and pump.swap AMM events, protocol-derived reserves, reference/executable price primitives, per-slot volume, and the parallel market-state engine.

M2 identified two correctness limitations:

1. **pump.swap reserve freshness:** the Pool account does not contain current reserves, so current reserve state comes from observed Buy/Sell events.
2. **exact executable pricing:** the current constant-product quote primitive may differ from exact on-chain integer rounding by a raw unit.

Fix these cleanly.

**Do not redesign M2. Do not start M3. Do not add trading.**

## 1. Read before coding

Read completely:

- `NEURONE_BLUEPRINT.md`
- `docs/MILESTONE_2_REPORT.md`
- `docs/SOLAMI_RESEARCH.md`
- `README.md`
- relevant `src/` and `tests/`
- current `task.md` if present
- relevant Neo Agent skills under `/home/xion/neo-agent`

Inspect:

```bash
git status
git log --oneline -10
```

Identify which Neo skills are relevant, which files implement pump.swap state, which implement pricing/`Ratio`, which tests cover them, and which authoritative protocol sources define the exact math.

Use the relevant Neo Agent skills and report which were actually used.

Do not modify `NEURONE_BLUEPRINT.md`.

## 2. Preserve architecture

Keep:

```text
Yellowstone
    ↓
Normalizer / Decoder
    ↓
hash(market)
    ↓
parallel shards
    ↓
MarketState
```

Do NOT introduce:

- a serial scanner;
- a central global market lock;
- per-market RPC polling;
- synchronous RPC on the hot path;
- database dependencies;
- Beam;
- transaction construction;
- wallet signing.

The fix must remain compatible with thousands of concurrent markets.

## 3. Fix A — pump.swap reserve state and freshness

Maintain explicit pump.swap reserve state:

```text
base_reserve
quote_reserve
reserves_known
last_reserve_slot
last_reserve_timestamp
last_reserve_signature
```

A valid Buy/Sell event supplies the latest observed pool reserves and should update the market state.

Do not replay the entire historical trade sequence when the event already provides authoritative pool reserves.

Represent the state distinctly as:

```text
UNKNOWN
KNOWN + FRESH
KNOWN + STALE
```

or an equivalent clean model.

Provide a deterministic freshness primitive such as:

```text
is_reserve_state_fresh(...)
```

The stale threshold must be infrastructure/configuration, not an M3 strategy rule.

Preserve per-market ordering. Older slot/write-version data must not overwrite newer reserve state. Duplicate/replayed events remain idempotent. Different pools remain independently parallel.

## 4. Do not solve freshness with RPC polling

Do NOT poll every pool with RPC.

Yellowstone remains the primary real-time state source.

RPC is acceptable only for validation/tests/fixture generation and must not become the runtime reserve-refresh mechanism.

## 5. Fix B — exact protocol executable quote engine

Separate:

```text
OBSERVED MARKET STATE
        ↓
EXACT QUOTE ENGINE
        ↓
future qualification / arming
        ↓
future execution
```

Create a clean protocol-specific quote abstraction, conceptually:

```text
quote_buy(market_state, input_amount)
quote_sell(market_state, input_amount)
```

The exact Rust API is up to the implementation.

For each supported venue where executable quotes are claimed:

- use exact integer protocol math;
- include relevant fees;
- use exact integer division;
- use exact rounding direction;
- enforce relevant constraints;
- avoid floating point;
- avoid UI-unit conversions in the hot path;
- keep pump.fun and pump.swap formulas separate when their mechanics differ.

## 6. Verify exact protocol math

Do not rely on memory.

Use authoritative current sources, starting from the official pump.fun IDLs and program/instruction definitions already used by M2.

The goal is:

```text
Rust quote == protocol integer result
```

for deterministic known cases.

Do not merely reproduce the existing M2 approximation.

## 7. Exact quote tests

Add deterministic fixtures for both venues covering:

- normal buy;
- normal sell;
- tiny input;
- large input;
- fee-bearing trade;
- zero input;
- zero reserves;
- insufficient reserves;
- non-even integer division;
- rounding boundaries;
- maximum safe integer values;
- overflow protection.

Assert exact integer equality, not epsilon-based approximate equality.

Where possible, compare against known on-chain event outputs.

## 8. Quote result

Expose enough information for later M3/M4:

```text
input_amount
gross_output
fee_amount
net_output
effective_price
venue
side
valid / invalid
reason
```

Do not add strategy fields such as TP, SL, target multiple, capital allocation, or slippage policy.

## 9. Fresh executable-state concept

Make it possible for later milestones to distinguish:

```text
market exists
```

from:

```text
market has current executable state
```

A simple representation may distinguish:

```text
identity_known
reserves_unknown
reserves_known
reserves_stale
quote_supported
quote_unsupported
```

Do not build the M3 safety/qualification state machine.

## 10. Venue separation

Keep pump.fun and pump.swap behavior explicit.

Pump.fun:

- bonding-curve state comes from the bonding-curve account;
- trade events also contain reserve information.

Pump.swap:

- pool identity comes from the Pool account;
- current reserve state comes from observed swap events.

Do not force both into an incorrect identical state model.

## 11. Real mainnet validation

After implementation, validate against authenticated Solami Yellowstone again.

Verify:

### pump.fun
- real trade event;
- decoded reserves;
- quote calculation;
- reserve/state update.

### pump.swap
- real buy event;
- real sell event;
- pool reserve update;
- freshness tracking;
- exact quote result.

Never log or persist credentials.

Normal tests must remain network-independent.

## 12. Cross-check quote calculations

Where possible:

```text
observed swap input
observed output
observed reserves
observed fee
        ↓
quote engine
        ↓
expected output
```

Expected output must match the actual protocol event exactly when the same state/input semantics apply.

If an event cannot provide an exact comparison because of protocol-specific semantics, document why.

## 13. Performance

Measure:

```text
quote_buy p50/p95/p99
quote_sell p50/p95/p99
reserve-update p50/p95/p99
```

Rerun important M2 benchmarks to detect regressions.

Keep the quote engine allocation-light and hot-path suitable.

## 14. Failure handling

Safely handle:

- zero reserves;
- zero input;
- insufficient liquidity;
- overflow;
- invalid fees;
- malformed state;
- stale reserves;
- unsupported quote mint;
- unsupported venue;
- invalid protocol state.

Return deterministic results/errors. Never panic on malformed market data.

## 15. Explicit non-goals

Do NOT implement:

- M3 volume filters;
- 5-minute qualification;
- accelerating-volume strategy;
- low-MC strategy;
- liquidity thresholds;
- safety qualification;
- pre-arming;
- capital arbitration;
- wallet signing;
- transaction construction;
- Beam;
- live buying/selling;
- TP/SL;
- frontend;
- LLM/narrative analysis;
- autonomous strategy changes.

M2.1 is correctness hardening only.

## 16. Tests

All M1/M2 tests must remain green.

Add tests for:

### Reserve state
- first swap establishes reserves;
- newer swap replaces reserves;
- older swap cannot overwrite newer state;
- duplicate swap is idempotent;
- stale state is detected;
- unknown state is represented;
- multiple pools remain independent.

### Quote engine
- exact buy outputs;
- exact sell outputs;
- exact fees;
- exact rounding;
- zero/invalid inputs;
- boundary values;
- overflow safety;
- venue-specific formulas.

### Parallelism
Prove reserve/quote logic does not introduce a serial/global bottleneck.

## 17. Documentation

Create:

```text
docs/MILESTONE_2_1_REPORT.md
```

Document:

1. original issue;
2. root cause;
3. implementation;
4. reserve-state model;
5. freshness model;
6. exact quote formulas;
7. protocol sources;
8. integer/rounding behavior;
9. tests;
10. live validation;
11. performance;
12. limitations;
13. recommended M3.

For each issue explicitly state:

```text
FIXED
PARTIALLY FIXED
REMAINS A LIMITATION
```

Do not modify `NEURONE_BLUEPRINT.md`.

## 18. Verification

Run:

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets
cargo build --release
```

Run relevant benchmarks.

Inspect the final diff for:

- accidental secrets;
- unnecessary dependencies;
- debug logging;
- RPC polling;
- serial bottlenecks;
- global locks;
- unrelated M3 code;
- blueprint changes.

## 19. Git

Commit completed M2.1 work with a clear commit message.

Do not start M3 automatically.

Stop after M2.1.

## Final response format

Return exactly:

```text
M2.1 status:
Reserve-state fix:
Reserve freshness:
Exact quote engine:
Pump.fun:
Pump.swap:
Exact rounding:
Live validation:
Tests:
Clippy:
Build:
Performance:
Known limitations:
Neo Agent skills used:
Git commit:
M3 readiness:
```

Clearly distinguish verified live behavior, deterministic fixture/test evidence, and remaining assumptions.

Never claim exact protocol parity unless tests establish it.

# DEFINITION OF DONE

- [ ] pump.swap reserve state is explicit and freshness-aware.
- [ ] Older/replayed events cannot overwrite newer reserve state.
- [ ] No per-market RPC polling was introduced.
- [ ] Exact protocol-specific quote engine exists.
- [ ] Buy and sell calculations use integer arithmetic.
- [ ] Fees are represented correctly.
- [ ] Exact rounding behavior is tested.
- [ ] Boundary/overflow cases are tested.
- [ ] Real mainnet events validate the implementation.
- [ ] Existing M1/M2 tests remain green.
- [ ] New correctness tests pass.
- [ ] Clippy clean.
- [ ] Release build succeeds.
- [ ] Performance regression is measured.
- [ ] `docs/MILESTONE_2_1_REPORT.md` exists.
- [ ] Neo Agent skills were actually inspected and used.
- [ ] No M3/trading/Beam logic was introduced.
- [ ] Work is committed.
- [ ] Work stops.
