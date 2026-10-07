# Task: Pump.fun + PumpSwap Documentation-First Reconciliation Investigation

## Objective

Run a fresh, thorough investigation of **Pump.fun** and **PumpSwap**, in this order:

1. Thoroughly search and study the **official Pump.fun and PumpSwap documentation/source material first**.
2. Run a **new empirical/code/data investigation** of Pump.fun and PumpSwap.
3. Make a **technical reconciliation** between the documentation findings and the fresh investigation findings.

This is **not an executive summary**. The deliverable must be a technical reconciliation based directly on the documentation research and the new investigation.

Do not jump into implementation or modify production quote/state logic.

## Scope

Focus on:
- exact BUY/SELL quote formulas
- reserve semantics
- virtual reserves / effective reserves
- fee calculation and fee regimes
- protocol fees and creator fees
- input/output semantics
- integer rounding (`floor`, `ceil`, adjustments)
- `-1` / `+1` terms
- Pump.fun bonding-curve state
- PumpSwap AMM pool state
- event fields and historical-state reconstruction
- versioned/extended account layouts
- historical versus current protocol behavior
- what Yellowstone can and cannot reconstruct deterministically

Stay tightly focused on these questions.

# Part 1 — Documentation-First Research

Before the new investigation, thoroughly search the **official first-party sources** for both protocols.

### Pump.fun
Prioritize:
- official Pump program repository/documentation
- official IDL
- official instruction docs
- official SDK/source where formulas or fee behavior are defined
- official account/state definitions
- official event definitions
- official breaking-change/version documentation

### PumpSwap
Prioritize:
- official PumpSwap repository/documentation
- official IDL
- official instruction docs
- official SDK/source
- official account/state definitions
- official event definitions
- official fee documentation
- official documentation on virtual quote reserves and pool versions

Use current official sources and historical/older layout documentation where relevant.

## Documentation extraction requirements

For every important finding, record:
- source/file/document
- relevant instruction/account/event
- exact field names
- exact formula where documented
- fee parameters
- rounding direction
- state semantics
- whether it is current-only or explains historical layouts
- explicit versioning/backwards-compatibility behavior

Do not paraphrase away important mathematical details.

If an official source explicitly gives a formula, preserve it closely enough to compare against the empirical investigation.

If official sources conflict across versions, record the conflict rather than silently choosing one.

# Part 2 — Fresh Investigation

After the documentation pass, run a fresh investigation of the existing Pump.fun and PumpSwap evidence/code/data.

Re-run or reproduce the relevant investigation rather than relying only on the previous report.

## Pump.fun

### SELL
Investigate:
- reserve interpretation
- exact token → SOL calculation
- fee treatment
- rounding
- creator fee treatment
- protocol fee treatment
- whether the observed formula exactly reproduces TradeEvent/post-trade values

### BUY
Investigate each instruction separately:
- `buy_exact_sol_in`
- `buy_exact_quote_in`
- token-target `buy`

For each determine:
- exact input/output semantics
- formula
- fee base
- rounding
- `-1` / `+1` adjustments
- predicted vs observed output
- exact matches vs mismatches

Do not combine the three BUY variants unless the evidence proves equivalence.

## PumpSwap

### SELL
Investigate:
- raw base reserve
- raw quote reserve
- `virtual_quote_reserves`
- effective quote reserve
- fee regime
- creator/protocol fees where applicable
- exact rounding
- event-state vs account-state reconstruction
- historical pool layouts/version lengths

### BUY
Investigate:
- exact input semantics
- reserve semantics
- effective quote reserve
- fee calculation
- fee regimes
- rounding
- event fields
- whether exact historical state is observable

Independently investigate the observed regimes:
- `(25,5)`
- `(2,93)`
- `(20,5)`

Do not assume these are the same protocol configuration merely because they are pairs of fee numbers.

# Part 3 — Documentation ↔ Investigation Reconciliation

For every major question explicitly compare:

**A. What the official documentation says**

vs.

**B. What the fresh investigation observes**

vs.

**C. Whether they agree**

Use classifications such as:
- `CONFIRMED — docs and investigation agree`
- `DOCS EXPLAIN INVESTIGATION — previous uncertainty resolved`
- `INVESTIGATION SUPPORTS DOCS BUT STATE DEPENDENCY REMAINS`
- `DOCS AND INVESTIGATION DISAGREE`
- `DOCUMENTATION INSUFFICIENT`
- `INVESTIGATION INSUFFICIENT`
- `HISTORICAL STATE NOT RECONSTRUCTABLE FROM OBSERVED DATA`
- `FORMULA KNOWN, REQUIRED INPUT STATE UNKNOWN`

Do not force reconciliation where evidence does not support one.

# Specific Reconciliation Questions

## Pump.fun

1. Are the observed `-1` / `+1` terms actually documented by first-party sources?
2. Which exact BUY instruction uses which formula?
3. Are `buy_exact_sol_in` and `buy_exact_quote_in` modeled with the correct fee-adjusted input?
4. Is token-target `buy` genuinely unresolved, or does official documentation explain the previous mismatch?
5. Are creator/protocol fees applied to the correct base?
6. Are mismatches caused by wrong formula, wrong fee state, wrong reserve state, wrong rounding, wrong instruction semantics, or missing historical state?
7. Does current official documentation/implementation explain the empirical match rates previously observed?

## PumpSwap

1. Does official documentation define effective quote reserves as raw quote reserve plus signed `virtual_quote_reserves`?
2. Does it state whether BUY and SELL both use effective quote reserves?
3. Are `virtual_quote_reserves` exposed in pool state and/or trade events?
4. If exposed in events, does that resolve the previous conclusion that historical virtual/adjustment state was unavailable?
5. What exactly explains the `(20,5)` residual and observed ±1 differences?
6. Are `(25,5)`, `(2,93)`, and `(20,5)` different fee configurations, historical states, or something else?
7. Does official documentation explain PumpSwap BUY sufficiently to determine the exact formula?
8. If PumpSwap BUY remains unresolved, identify the exact missing variable/state instead of simply labeling the whole BUY path unknown.
9. Can the exact historical trade be reconstructed from Yellowstone alone for each supported path?

# Required Output

Produce a technical reconciliation report, not an executive summary.

Recommended structure:

## 1. Documentation Findings
### Pump.fun
### PumpSwap

## 2. Fresh Investigation Findings
### Pump.fun SELL
### Pump.fun BUY
- buy_exact_sol_in
- buy_exact_quote_in
- buy
### PumpSwap SELL
### PumpSwap BUY

## 3. Documentation ↔ Investigation Reconciliation
Use a table where useful:

| Topic | Official documentation | Fresh investigation | Reconciliation | Status |
|---|---|---|---|---|

Do not compress the table so much that mathematical details disappear.

## 4. Resolved Questions

List exactly what the new work resolves compared with the previous investigation.

## 5. Remaining Contradictions / Unknowns

For every unresolved issue state:
- what is known
- what is unknown
- what evidence was checked
- what exact missing state/information prevents closure

## 6. Protocol-Parity Consequences

Only state consequences directly supported by the reconciliation.

Distinguish:
- exact and production-certifiable
- exact only under observable-state conditions
- empirically matching but not formally documented
- not certifiable

## 7. Source Register

List the official Pump.fun and PumpSwap sources actually inspected, including repository/file/document names and relevant sections/fields.

# Important Rules

1. **Documentation first.** Do not begin by reading only the old investigation and then searching for docs that fit it.
2. Use first-party sources wherever possible.
3. Third-party sources are supplementary only and must be clearly labeled.
4. Do not treat an SDK implementation as authoritative merely because it matches observed data; distinguish implementation evidence from formal protocol documentation.
5. Do not silently overwrite or reinterpret previous findings.
6. If new evidence changes a previous conclusion, explicitly explain why.
7. Preserve exact formulas and integer arithmetic.
8. Do not use floating-point approximations for protocol formulas.
9. Separate current protocol behavior from historical behavior.
10. Separate observable state from inferred state.
11. Do not turn an unresolved issue into a speculative explanation.
12. Do not make production code changes as part of this task unless needed to reproduce an investigation; isolate and identify any such changes.
13. Do not produce an executive summary as the primary deliverable.
14. Make the report detailed enough that another engineer can trace each reconciliation claim back to both official documentation and fresh empirical evidence.

## Final Deliverable

Write the completed report to:

`task.md`

If the repository already has a suitable investigation/report directory, use it only if consistent with the existing project structure; otherwise use the requested `task.md` location.

Do not stop after documentation research. The task is complete only after:

**official docs → fresh investigation → reconciliation → task.md**
