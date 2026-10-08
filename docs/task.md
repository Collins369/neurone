# Neurone M4 — Strategy V1 Implementation Task

Implement **M4 — Strategy V1** in the Neurone Rust codebase thoroughly, critically, and meticulously.

## Mandatory first steps

1. Read `NEURONE_BLUEPRINT.md` completely.
2. Inspect the entire current repository.
3. Inspect the current M1/M2/M3 implementation and tests.
4. Inspect relevant Neo Agent skills.
5. Inspect current M3 reports/docs, especially the codebase audit, reserve/invalidation work, late-market semantics, and PumpSwap postTokenBalances bootstrap.
6. Trust current code over stale reports when they differ.

The blueprint remains the architectural source of truth.

Before coding, audit what M3 actually exposes at runtime: `MarketState`, reserve/liquidity state, MCAP inputs, rolling volume, market identity, buy/sell quote availability, token authorities, creation/age information, stale/invalidated semantics, sharding, transaction success handling, and quote pre-state assumptions.

There is a historical audit finding that `TransactionUpdate.success` may not be consumed by the runtime and that some pre-state corroboration exists only in `src/validate.rs`. Verify whether this is still true. If a genuine correctness gap blocks safe M4, fix the smallest necessary prerequisite and document it. Do not silently expand scope.

---

# Locked M4 strategy

M4 converts continuously maintained M3 market state into a deterministic `QUALIFIED` / not-qualified decision.

M4 stops at qualification. It does **not** arm, execute, sign, submit, manage positions, or implement TP/SL.

## 1. Supported markets

Support:

- Pump.fun bonding curves
- PumpSwap pools

Reject unsupported/malformed market identity.

## 2. Liquidity

Hard minimum:

```text
liquidity >= $2,000
```

Inclusive boundary:

```text
$1,999 → reject
$2,000 → pass
```

Use maintained deterministic state. Unknown/stale/invalid liquidity fails closed.

## 3. Market cap

Locked inclusive range:

```text
$2,000 <= MCAP <= $10,000
```

Therefore:

```text
$1,999 → reject
$2,000 → pass
$10,000 → pass
$10,001 → reject
```

Use the existing deterministic MCAP calculation if present. Do not invent a conflicting formula.

## 4. Rolling 5-minute volume

Only volume criterion:

```text
rolling 5-minute volume >= $1,000
```

This means: at the current instant, did the market accumulate at least $1,000 during the immediately preceding rolling five-minute interval?

It does **not** mean the token must be five minutes old.

Example:

```text
token age = 2 minutes
rolling 5m volume = $1,250
→ pass
```

Implement an incremental bounded rolling-volume structure. Do not rescan trade history on every event.

### Explicitly excluded

Do **not** implement volume acceleration, volume-rate acceleration, previous-window comparisons, or acceleration thresholds.

The only volume gate is the rolling 5-minute threshold.

## 5. New-token / freshness filter

The strategy targets mostly genuinely new tokens.

Do not equate first observation by Neurone with token creation.

Derive actual age from chain-proven creation information where safely available.

Maintain only necessary information such as creation slot/time and age.

A maximum age must be configurable, but **do not invent a numeric value** if the project has not specified one.

If age cannot safely be established for a market type, fail closed or explicitly document the limitation.

## 6. Anti-pump/dump filter

Avoid old tokens that have already pumped and dumped back into the $2k–$10k MCAP range.

Maintain deterministic extrema where safely available, such as:

- peak MCAP
- peak price
- peak liquidity

Use them to detect excessive retracement.

The maximum allowed drawdown must be configurable. Do not invent its numeric value if unspecified.

If required history cannot safely establish this condition, document the limitation and fail closed rather than inventing a heuristic.

## 7. Deterministic maliciousness / safety

Create a deterministic safety/eligibility layer and fail closed.

Investigate and implement checks actually provable from current Neurone data for:

### Token
- mint validity
- mint authority
- freeze authority
- valid mint/account structure
- suspicious control configuration where deterministically observable

### Market
- valid Pump.fun/PumpSwap structure
- valid/current reserves
- reserve consistency
- no stale state
- no invalidated state
- impossible/contradictory state rejection

### Trading
- executable buy route
- executable sell route
- valid quotes
- valid reserves
- sane price impact
- sane fees/economics
- no impossible quote

Do not claim these checks prove a token cannot rug. Unknown required information must reject/remain unqualified.

No HTTP APIs, RPC polling, LLMs, narrative analysis, or DB queries in the hot path.

## 8. Buy executability

Require a deterministic valid buy quote/route from current maintained state.

Reject unavailable reserves, stale/invalid state, unsupported routes, quote failure, or unknown required conditions.

Use existing exact integer quote machinery; avoid floating point in quote decisions.

## 9. Sell executability

Require a deterministic valid reverse sell route.

A market must not qualify merely because it is buyable.

## 10. Execution economics

Evaluate existing deterministic economics before qualification, including whatever the current architecture can safely establish for:

- expected buy cost
- expected sell proceeds
- fees
- price impact
- slippage constraints
- minimum executable conditions

Do not invent arbitrary profitability thresholds. If none are currently defined, make the mechanism configurable and document the missing parameter.

No transaction is sent.

## 11. State validity

Conceptually:

```text
Known + current → continue
Unknown          → not qualified
Stale            → not qualified
Invalidated      → not qualified
```

Temporary Unknown/Stale must not permanently reject a market; fresh authoritative state should allow reevaluation.

## 12. Qualification lifecycle

M4 implements:

```text
OBSERVING → QUALIFIED
```

Do not implement M5 behavior:

- QUALIFIED → ARMED
- transaction construction/signing
- Beam
- capital arbitration
- position management
- TP/SL
- exits

Preserve the existing lifecycle rather than creating a competing state machine.

## 13. Deduplication

Prevent the same consumed opportunity from repeatedly qualifying.

Use deterministic identifiers and strategy versioning without introducing database hot-path dependency.

Temporary failure must not permanently consume a market.

## 14. Configuration

Use explicit strategy configuration.

Locked values:

```yaml
min_liquidity_usd: 2000
min_market_cap_usd: 2000
max_market_cap_usd: 10000
rolling_volume_window_seconds: 300
min_rolling_volume_usd: 1000
```

Freshness age and anti-pump/dump drawdown remain configurable but must not receive arbitrary hidden numeric values.

No volume acceleration configuration.

## 15. Parallel hot path

Use the existing sharded architecture:

```text
Yellowstone event
  ↓
normalization
  ↓
hash(pool/mint)
  ↓
owning shard
  ↓
update one MarketState
  ↓
evaluate M4 for that market
  ↓
QUALIFIED / remain OBSERVING
```

Do not create a global full-market scan.

Do not create one async task/thread per market or trade.

Prefer evaluation within the owning shard.

Avoid global locks, cross-shard synchronization, blocking I/O, RPC/HTTP, filesystem access, DB access, large protobuf retention, unnecessary allocations, and unnecessary serialization.

## 16. Rolling-volume implementation requirements

Maintain rolling 5m volume incrementally with a bounded ring/bucket structure or equivalent.

Requirements:

- O(1) or amortized O(1) update
- no historical scan per trade
- deterministic expiration
- exact boundary semantics documented
- integer arithmetic where possible
- bounded memory per market

Be meticulous about timestamp/slot/window boundaries.

## 17. Rejection reasons

Add deterministic diagnostics using project naming conventions, covering at least:

```text
UNSUPPORTED_MARKET
TOKEN_TOO_OLD
INSUFFICIENT_FRESHNESS_DATA
ALREADY_PUMPED
LOW_LIQUIDITY
LOW_5M_VOLUME
MCAP_TOO_LOW
MCAP_TOO_HIGH
UNSAFE_TOKEN
UNSAFE_MARKET
BUY_UNAVAILABLE
SELL_UNAVAILABLE
BAD_EXECUTION_ECONOMICS
STATE_UNKNOWN
STATE_STALE
STATE_INVALIDATED
ALREADY_CONSUMED
```

Avoid noisy per-rejection logs; prefer structured diagnostics/counters.

## 18. Telemetry

Add non-blocking M4 telemetry:

```text
markets_evaluated
markets_qualified
rejected_unsupported
rejected_too_old
rejected_already_pumped
rejected_low_liquidity
rejected_low_5m_volume
rejected_mcap
rejected_safety
rejected_buy
rejected_sell
rejected_execution
rejected_state
rejected_consumed
```

Measure qualification evaluation latency where consistent with existing telemetry.

Telemetry must never block execution.

## 19. Tests

Thorough deterministic tests are mandatory.

### Liquidity
- 1999 reject
- 2000 pass
- 2001 pass

### MCAP
- 1999 reject
- 2000 pass
- 10000 pass
- 10001 reject

### Rolling volume
- 999 reject
- 1000 pass
- 1001 pass
- multiple trades accumulate to threshold
- 2-minute-old token with $1000+ rolling volume passes
- expired volume removed
- exact boundary timestamp behavior
- zero/sparse volume
- high trade count
- bounded rolling structure

### Freshness
- valid new token
- too old
- unavailable creation information
- late observation does not falsely redefine age
- pumped then dumped
- acceptable drawdown
- insufficient history handled safely

### Safety
Every implemented safety condition gets pass/fail/unknown tests as applicable.

### State
- Known/current
- Unknown
- Stale
- Invalidated
- recovery after authoritative update

### Buy/sell
- valid buy
- invalid buy
- valid sell
- invalid sell
- quote failure
- unavailable state

### Economics
- valid
- invalid
- boundary behavior

### Dedup
- consumed opportunity cannot repeatedly qualify
- temporary failure can qualify after state changes

### Parallelism
Independent markets cannot contaminate each other.

### Determinism
Same event sequence/state produces same decision.

## 20. Benchmark

Benchmark M4 independently and under realistic concurrent event load.

Measure:

- per-market evaluation latency
- rolling-volume update latency
- throughput
- p50/p95/p99
- allocations where measurable
- shard behavior under concurrent updates

Do not optimize blindly. Compare against M3 baseline and ensure M4 does not become a bottleneck.

## 21. Validation

After implementation:

1. cargo fmt/check
2. clippy
3. release build
4. complete test suite
5. M4-specific tests
6. existing live Solami validation
7. live observation of M4 telemetry/qualification

Do not enable live trading or send real transactions.

## 22. Strict no-go scope

Do not add:

- volume acceleration
- LLM/social/narrative analysis
- DexScreener/API polling
- RPC polling
- DB hot-path persistence
- Beam
- transaction signing/submission
- capital management
- TP/SL
- frontend
- ShredDirect
- Blur
- Mirage
- Index Engine
- Webhooks
- dynamic token-account firehose
- unrelated refactors

If a prerequisite is genuinely required, prove it, make the smallest safe change, and report it.

## 23. Final adversarial review

Before declaring M4 complete, verify:

### Correctness
- thresholds exactly match specification
- 2-minute token with $1k+ rolling 5m volume can qualify
- rolling window truly rolls
- old buckets expire correctly
- pumped/dumped tokens cannot slip through where the data supports detection
- Unknown/Stale/Invalidated cannot qualify
- unsupported markets cannot qualify
- unsafe/unprovable safety conditions cannot qualify
- buy and sell paths are required
- consumed opportunities cannot repeatedly qualify

### Architecture
- no hidden serial market scan
- no global lock
- no blocking I/O
- no RPC/HTTP
- no unnecessary allocation/serialization
- state remains sharded and parallel

### Protocol
- quote assumptions are verified against current implementation
- transaction success handling is correct
- state-dependent quotes use authoritative/current state
- Pump.fun and PumpSwap semantics are respected

### Performance
- M4 does not materially degrade M3 latency
- rolling volume is bounded
- telemetry is cheap/non-blocking

### Safety
- unknown required data fails closed
- no heuristic is presented as a guarantee
- no hidden invented thresholds

Fix genuine findings before completion.

## Deliverables

Produce:

1. M4 implementation
2. tests
3. configuration changes
4. necessary documentation
5. benchmark results
6. live validation results
7. concise M4 implementation report

Report:

```text
M4 status
Files changed
Architecture changes
Strategy gates
Safety checks
Configuration
Tests
Benchmarks
Live validation
Prerequisite fixes
Known limitations
Intentionally unimplemented items
```

Final standard:

**correctness → deterministic behavior → parallelism → fail-closed safety → measurement → low overhead → minimal scope.**

Do not confuse more code with more thoroughness.
