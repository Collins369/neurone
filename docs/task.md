# Neurone — M2.2 Investigation Task
## Buy-Quote Parity: Investigate First, Propose the Fix, Do Not Implement It

You are DeepSeek/Codex operating inside the Neurone repository.

**This is investigation only. Do NOT implement the production fix.**

## 1. Read first

Read, in order:
- `NEURONE_BLUEPRINT.md`
- `docs/MILESTONE_1_REPORT.md`
- `docs/MILESTONE_2_REPORT.md`
- `docs/MILESTONE_2_1_REPORT.md`
- current `src/quote.rs`, `src/market.rs`, `src/shard.rs`, protocol decoders, reserve-state code, fee handling, parity tests, and Solami integration
- relevant Neo Agent skills under `/home/xion/neo-agent`
- existing Solami/pump research in the repo

Do not modify `NEURONE_BLUEPRINT.md` or historical milestone reports.

## 2. Problem

M2.1 made reserve state correct and introduced an integer quote engine. SELL parity is exact. BUY parity remains unresolved:

### pump.fun BUY
- current implementation is constant-product-style on net input;
- exact on-chain integer parity was not established;
- observed residual is roughly one lamport's worth of base tokens.

### pump.swap BUY
- current implementation reproduces observed buys to `<1 ppm`;
- exact integer parity has not been established.

M2.1 also leaves fee bps as a caller parameter rather than authoritative observed Global/GlobalConfig state.

The objective is to determine the actual root cause and propose the smallest correct fix.

## 3. Investigation rules

Do **not** immediately modify `src/quote.rs`.

Do not:
- guess the formula;
- copy a third-party formula and call it exact;
- assume pump.fun and pump.swap use the same BUY math;
- call `<1 ppm` good enough;
- claim exact parity without proof.

If exact behavior cannot be proven, state that explicitly.

Clearly classify claims as `VERIFIED`, `OBSERVED`, `INFERRED`, `HYPOTHESIS`, or `UNRESOLVED`.

## 4. pump.fun BUY investigation

Investigate current:
- official IDLs and program docs;
- SDK/reference implementations;
- instruction argument semantics;
- bonding-curve fields, virtual/real reserves and completion/caps;
- protocol and creator fees;
- dynamic fee configuration;
- fee direction and inversion;
- floor/ceil/truncation at every step;
- any `-1` adjustment;
- whether math is derived from SOL-in or token-out;
- real-token-reserve cap;
- exact fee-vs-constant-product ordering;
- version differences.

If source is unavailable, use defensible alternatives: labelled mainnet samples, instruction/inner-instruction data, account state before/after BUYs, logs, and if necessary technically feasible disassembly/reverse engineering.

Separate documented behavior from observation and inference.

## 5. pump.swap BUY investigation

Treat it as a separate protocol. Investigate:
- pool reserve semantics;
- LP/protocol/creator/dynamic fees;
- fee configuration and whether it changes;
- input-vs-output fee direction;
- fee ordering;
- integer rounding;
- constant-product calculation;
- reserve update semantics;
- whether BuyEvent values are pre- or post-trade;
- instruction arguments versus event values;
- constraints and protocol versions.

Use real mainnet BUY samples. Prefer hundreds or thousands where practical, partitioning by pool, reserve size, fee regime, trade size, slot, market age, quote mint, decimals and configuration when useful.

## 6. Candidate formulas

Enumerate plausible candidates, including differences in:
- fee-before-CP vs fee-after-CP;
- floor vs ceil;
- fee inversion;
- `amount - 1`;
- reserve + input vs reserve + net input;
- real-reserve caps;
- one-unit corrections;
- fee split/order;
- pre-trade vs post-trade reserves.

For each candidate produce a table:

| Candidate | Mathematical sequence | Rounding | Samples | Exact matches | Max error | Failure pattern |
|---|---|---|---:|---:|---:|---|

Do not select a formula from a tiny sample.

## 7. Differential parity harness

Create temporary research tooling if needed. Compare:

`observed BUY -> observed state/inputs -> candidate formula -> predicted output -> observed output`

Measure:
- exact matches/mismatches;
- absolute and relative error;
- maximum error;
- error distribution;
- residual direction;
- residual versus reserve ratio, trade size, fee and pool/curve state.

The acceptance metric for an exact formula is **100% exact integer parity over the validated labelled fixture set**. `<1 ppm` is not exact parity.

## 8. Fee-state investigation

Determine whether `fee_bps` should remain a caller argument or become part of observed market state.

Investigate Global, GlobalConfig, pool configuration, dynamic fee state, event fee fields, BUY/SELL differences, and whether fees can change during a live market.

Identify the minimum state required for an authoritative quote. Do not implement it yet.

## 9. Verify M2.1

Reference concrete code paths and answer with evidence:
1. Is pump.fun BUY mathematically wrong?
2. Which operation is wrong?
3. Is fee calculation wrong?
4. Is rounding wrong?
5. Is reserve input wrong?
6. Is a real-reserve cap missing?
7. Is `-1` real/required?
8. Is pump.swap BUY wrong?
9. Which operation differs?
10. Is `<1 ppm` caused by rounding, fee interpretation, reserve semantics, or something else?
11. Are there protocol-version differences?
12. Are current tests asserting the wrong invariant?

## 10. Proposed solution — do not implement

After investigation, propose the smallest correct fix covering:

### Data model
Exactly what `MarketState` fields must change, if any.

### Quote engine
Exact functions/formulas to change, with integer operation sequence.

### Fee state
Where authoritative fee values come from.

### Rounding
Every floor/ceil/truncation rule.

### Versioning
How protocol/version differences are detected and selected.

### Safety
Behavior for unknown/stale reserves, unknown/stale fees, overflow, zero output, insufficient liquidity and inconsistent state.

### Performance
Expected hot-path cost. Must preserve parallelism, no global lock, no serial scan, no RPC polling, no blocking network, no LLM, no strategy logic.

### Tests
Deterministic formula/edge/overflow/fee/rounding/reserve tests; real mainnet fixtures; large-sample differential parity; SELL regression tests.

If 100% exact parity cannot honestly be achieved, explain precisely what remains unknown and what evidence is missing.

## 11. Adversarial review

Try to disprove the proposed formula:
- overfitting;
- pre/post-trade event interpretation;
- changing fee regimes;
- version differences;
- reserve-update timing;
- transaction ordering;
- non-SOL quote markets;
- bonding-curve completion boundary;
- hidden constraints;
- net versus gross event amounts;
- apparent one-unit corrections caused by reconstruction artifacts.

Do not stop at the first fitting formula.

## 12. Deliverable

Create:
`docs/MILESTONE_2_2_INVESTIGATION_REPORT.md`

Required sections:
1. Executive conclusion
2. Current M2.1 behavior
3. pump.fun BUY investigation
4. pump.swap BUY investigation
5. Evidence sources
6. Candidate formulas
7. Differential parity results
8. Fee-state investigation
9. Root cause
10. Proposed implementation
11. Required data-model changes
12. Test/acceptance plan
13. Risks and unresolved questions
14. Recommendation: `FIX NOW` or `MORE INVESTIGATION REQUIRED`

## 13. Production scope prohibition

Allowed only:
- temporary research scripts;
- fixture collection;
- analysis tooling;
- investigation-only tests.

Do NOT implement production quote changes or begin M3. No filters, qualification, arming, capital arbitration, trading, transaction construction/signing, Beam, TP/SL or frontend.

## 14. Architecture constraints

Preserve:

`SOLANA -> SOLAMI YELLOWSTONE -> EVENT NORMALIZER -> hash(pool/mint) -> PARALLEL SHARDS -> MARKET STATE -> QUOTE ENGINE -> future qualification/arming`

No global market lock, serial token scan, per-market RPC polling, blocking hot-path network calls, LLM or external API calls in the quote hot path.

## 15. Git

Do not modify the blueprint or historical reports. Avoid unrelated files. Clean temporary artifacts unless needed for reproducibility.

At the end:
1. run relevant tests;
2. run formatting/linting as appropriate;
3. inspect `git diff`;
4. report every modified file;
5. commit only if consistent with the existing milestone workflow.

Suggested commit:
`research: investigate buy quote parity`

## 16. Final response

Report:
- investigation performed;
- sources consulted;
- number/type of real BUY samples;
- formulas tested;
- exact parity results;
- pump.fun root cause;
- pump.swap root cause;
- fee-state conclusion;
- proposed production fix;
- files that would need changing;
- tests required;
- unresolved uncertainty;
- `FIX NOW` or `MORE INVESTIGATION REQUIRED`.

Then STOP. Do not implement the proposed fix until explicitly authorized.
