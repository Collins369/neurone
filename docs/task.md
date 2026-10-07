# Neurone — M2.2A Protocol Parity Closure
## Finish the BUY/SELL protocol investigation, then STOP

### Role
You are DeepSeek/Codex operating inside the Neurone repository.

This task is the **final investigation/closure pass for M2.2**.

We have already investigated the remaining quote-parity problems. Your job now is to finish the evidence needed to close the protocol layer.

**Do not implement the production fix in this task.**

There are exactly two objectives:

1. Confirm pump.fun BUY parity at a sufficiently large sample.
2. Resolve pump.swap reserve semantics, effective reserves, fee regimes, and exact BUY/SELL parity.

Once the acceptance criteria are satisfied — or a clearly documented protocol limitation makes them impossible — produce the final report and STOP.

Do not start M3.

---

# 1. Source of truth

Before working:

1. Read `NEURONE_BLUEPRINT.md`.
2. Read:
   - `docs/MILESTONE_1_REPORT.md`
   - `docs/MILESTONE_2_REPORT.md`
   - `docs/MILESTONE_2_1_REPORT.md`
   - `docs/MILESTONE_2_2_INVESTIGATION_REPORT.md`
3. Inspect:
   - `src/quote.rs`
   - `src/market.rs`
   - `src/shard.rs`
   - protocol decoders
   - existing parity tests
   - existing research tooling
4. Inspect `/home/xion/neo-agent` and use the relevant protocol-research, architecture, adversarial-test, and quality-gate skills.
5. Consult current authoritative Pump documentation/IDL/reference sources where required.
6. Use real mainnet data through the existing Solami/RPC research path.

Do not modify `NEURONE_BLUEPRINT.md`.
Do not rewrite historical reports.

---

# 2. What M2.2 already established

## pump.fun

M2.1 incorrectly treated BUY as one generic operation.

M2.2 identified three BUY instruction types:

- `buy_exact_sol_in`
- `buy_exact_quote_in`
- `buy`

Observed candidate behavior:

```text
Exact-in:
tokens_out = floor(virtual_token_reserves * (input - 1) /
                   (virtual_sol_reserves + input - 1))

Token-target buy:
sol_required = ceil(virtual_sol_reserves * token_target /
                    (virtual_token_reserves - token_target))
```

Observed:
- `buy_exact_sol_in`: 20/20 exact
- `buy_exact_quote_in`: 30/32 exact
- `buy`: 50/54 exact

The `-1` behavior is observed and appears to be real protocol behavior, but the sample is not sufficient to certify 100% parity.

## pump.swap

M2.2 found the M2.1 model was too simplistic:

- M2.1's "exact SELL" claim was based on only 2 samples.
- Over 646 real sells, the simple formula was exact only 405/646.
- Some pools showed `lp_bps=2`, `protocol_bps=93`, versus older/documented `20/5`.
- Residuals can be much larger than ordinary rounding.
- `virtual_quote_reserves` is a likely component of effective quote reserves.
- A sampled Pool account was 260 bytes while the published layout appeared different.
- BUY candidate grid had 0/242 exact.

PumpSwap therefore remains unresolved.

---

# 3. Objective A — close pump.fun parity

Do not change production quote code.

Use research tooling/fixtures to establish whether the proposed formulas are actually protocol-exact.

## Required sample

Collect, where available:

- at least **1,000 `buy_exact_sol_in`** events;
- at least **1,000 `buy_exact_quote_in`** events;
- at least **1,000 `buy`** events.

If one instruction cannot realistically reach 1,000 samples, state the actual available population and why.

Partition samples by useful dimensions:

- trade size;
- curve state;
- market age;
- fee regime;
- completion state;
- quote mint;
- bonding-curve boundary;
- unusual reserve values.

Use correct pre-trade state reconstruction. M2.2 observed that TradeEvents represent post-trade state, so do not use post-trade reserves as pre-trade inputs.

## Test

For each instruction calculate:

- exact matches;
- mismatches;
- maximum absolute error;
- relative error;
- residual direction;
- residual distribution.

Acceptance:

> **100% exact integer parity across the validated labelled sample.**

If not 100%, determine whether the mismatch is formula, reserve reconstruction, fee handling, instruction semantics, protocol version, data quality, or another identifiable cause.

Do not call a formula exact if it is not exact.

## Boundary tests

Explicitly test:

- very small buys;
- large buys;
- near-completion bonding curves;
- unusual virtual reserves;
- real-token-reserve boundary;
- zero/near-zero remaining token supply;
- non-SOL quote markets where applicable.

Determine whether the real-token-reserve cap ever affects observed BUY behavior.

---

# 4. Objective B — close pump.swap parity

This is the main unresolved investigation.

Do not guess the formula.

## B1. Resolve Pool layout/version

Investigate the observed 260-byte Pool account versus the currently published layout.

Determine:

- exact discriminator/header size;
- exact field offsets;
- exact field sizes;
- which fields exist in the sampled 260-byte account;
- whether this is an older Pool version;
- whether fields were appended in newer versions;
- whether Neurone needs version-aware decoding.

Produce a clear layout table.

Do not silently reinterpret bytes.

## B2. Resolve reserve semantics

Collect **dense consecutive event chains per pool**.

Target:

> at least **200 pools × 10 or more consecutive BUY/SELL events**, where data availability permits.

For each chain determine whether event reserve fields are pre-trade, post-trade, or another defined state.

Use consecutive state transitions, not sparse unrelated transactions.

For every transition verify, where protocol semantics permit:

```text
state_before + trade_delta = state_after
```

If the event contains post-trade reserves, reconstruct pre-trade reserves from the immediately preceding state.

Document this precisely.

## B3. Resolve virtual quote reserves

Investigate the official `NEGATIVE_VIRTUAL_QUOTE_RESERVES` behavior and how it affects effective quote reserves.

For each pool determine:

```text
raw quote reserve
virtual quote reserve
effective quote reserve
```

Verify whether the effective reserve is `raw + virtual` or another protocol-defined transformation.

Preserve signed `i128` semantics. Do not clamp negative virtual reserves to zero.

Treat the virtual-reserve hypothesis as a hypothesis until differential data proves it.

## B4. Resolve PumpSwap fee regimes

Determine why pools may expose different fee values.

Investigate:

- LP fee;
- protocol fee;
- creator/dynamic fee if applicable;
- fee configuration accounts;
- instruction-specific fees;
- pool-specific fees;
- version-specific fee behavior.

For sampled pools record:

```text
pool
program/version if known
lp_bps
protocol_bps
other fee fields
slot
source/provenance
```

Determine which fee values are authoritative for a quote.

Do not assume older documented `20/5` values are universal.

## B5. Derive PumpSwap SELL formula

Before BUY, establish SELL correctly.

Test candidate formulas using:

- correct pre-trade reserves;
- effective reserves;
- correct fee regime;
- correct rounding;
- exact integer arithmetic.

Goal:

> **100% exact parity across the validated labelled SELL sample.**

Do not preserve the M2.1 "exact" claim unless the larger sample supports it.

If multiple pool versions require different formulas, identify the version discriminator.

## B6. Derive PumpSwap BUY formula

Only after B1–B5 are understood.

Test candidates including, where applicable:

- fee-before-constant-product;
- fee-after-constant-product;
- LP/protocol fee split;
- effective quote reserves;
- reserve + input;
- reserve + net input;
- floor vs ceil;
- protocol-specific unit adjustments;
- pre-trade vs post-trade reserve interpretation.

Do not assume pump.fun BUY math applies to PumpSwap.

Use real labelled BUY events.

Acceptance:

> **100% exact integer parity across the validated labelled BUY sample.**

If a candidate fails, analyze the residual rather than lowering the acceptance standard.

---

# 5. Differential parity harness

Use or extend the existing research harness.

It must report, by venue/instruction/side:

- sample count;
- exact matches;
- mismatch count;
- maximum absolute error;
- maximum relative error;
- mean/median error where useful;
- residual direction;
- candidate formula;
- fee regime;
- reserve model;
- pool/version.

The final report must distinguish:

```text
100% exact
```

from:

```text
<1 ppm
```

or:

```text
very close
```

---

# 6. Fee-state conclusion

Determine the minimum authoritative fee state production `MarketState` must eventually contain.

M2.2 proposed:

```text
lp_bps
protocol_bps
creator_bps
fee_slot
fee_signature
```

Confirm, reject, or refine this proposal based on evidence.

Determine whether fee state should be directly observed from events, decoded from configuration accounts, derived from both, or be version-specific.

Do not implement this production change yet.

---

# 7. Final production contract

At the end define the minimum information required for an authoritative quote.

Intended shape:

```text
Yellowstone
    ↓
Event Normalizer
    ↓
Parallel Shards
    ↓
MarketState
    ↓
Protocol-aware Quote Engine
    ↓
Executable?
```

Protocol layer must remain:

- O(1) quote calculation;
- integer-only;
- allocation-free on hot path;
- no global lock;
- no serial market scan;
- no per-market RPC polling;
- no blocking network operation in quote path;
- no LLM;
- no strategy logic.

---

# 8. Hard stop conditions

STOP investigation when BOTH are true:

### pump.fun
Instruction-aware formulas have been validated against the largest defensible labelled dataset, preferably ≥1,000 samples per instruction, with exact integer parity.

### pump.swap
Pool layout/version, reserve semantics, effective reserves, fee regime, and BUY/SELL formulas have been resolved sufficiently to achieve exact parity on the validated labelled dataset.

If exact parity is impossible because the deployed protocol exposes insufficient information, document the precise blocker.

Do NOT investigate indefinitely.

A clear protocol limitation is a valid conclusion.

---

# 9. No production implementation

Allowed:
- research scripts;
- temporary analysis;
- fixture collection;
- parity harnesses;
- temporary decoder experiments;
- deterministic research tests.

Do NOT implement:
- production quote changes;
- production MarketState changes;
- M3 filters;
- qualification;
- arming;
- capital arbitration;
- trading;
- transaction construction;
- signing;
- Beam;
- TP/SL;
- frontend.

Do not modify `NEURONE_BLUEPRINT.md`.
Do not rewrite previous milestone reports.

---

# 10. Final report

Create:

`docs/MILESTONE_2_2A_PROTOCOL_PARITY_REPORT.md`

Required sections:

1. Executive conclusion
2. Pump.fun confirmation
3. PumpSwap Pool layout/version
4. PumpSwap reserve semantics
5. PumpSwap virtual reserves
6. PumpSwap fee regimes
7. PumpSwap SELL parity
8. PumpSwap BUY parity
9. Candidate formulas and rejection reasons
10. Differential parity statistics
11. Authoritative fee-state model
12. Final quote-state contract
13. Exact production changes required
14. Risks/unresolved items
15. Final recommendation

Use explicit labels:
- `VERIFIED`
- `OBSERVED`
- `INFERRED`
- `HYPOTHESIS`
- `UNRESOLVED`

Do not turn hypotheses into facts.

---

# 11. Git and cleanup

At completion:

1. Run relevant research tests.
2. Run formatting/linting where applicable.
3. Inspect `git diff`.
4. Report every modified file.
5. Remove temporary artifacts not needed for reproducibility.
6. Commit only investigation artifacts if consistent with repository workflow.

Suggested commit:

`research: close protocol parity investigation`

Then STOP.

---

# 12. Final response to the operator

Report concisely:

- pump.fun final parity result;
- PumpSwap layout result;
- PumpSwap reserve result;
- virtual reserve result;
- fee-regime result;
- SELL parity;
- BUY parity;
- sample counts;
- exact-match percentages;
- final formulas;
- remaining uncertainty;
- exact production files that will need changing;
- whether M2 quote correctness is now **CLOSED** or **BLOCKED**.

Do not implement anything after the report.
