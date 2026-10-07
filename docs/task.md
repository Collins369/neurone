# NEURONE — M2.2B FINAL PROTOCOL PARITY INVESTIGATION

## Mission
Investigation only. No production quote/state changes. No M3.

This is the final parity pass before implementation or explicit exclusions.

Investigate ONLY:
1. PumpSwap `(20,5)` SELL residual.
2. PumpSwap BUY semantics for `(2,93)` and `(20,5)`.
3. PumpSwap fee/version/creator-fee differences.
4. Pump.fun BUY certification for `buy_exact_sol_in`, `buy_exact_quote_in`, and token-target `buy`.
5. Whether every production-required field is obtainable from Yellowstone-observed state without hot-path RPC.

## Read first
- `NEURONE_BLUEPRINT.md`
- `docs/MILESTONE_2_REPORT.md`
- `docs/MILESTONE_2_1_REPORT.md`
- `docs/MILESTONE_2_2_INVESTIGATION_REPORT.md`
- `docs/MILESTONE_2_2A_PROTOCOL_PARITY_REPORT.md`
- relevant Neo Agent skills under `/home/xion/neo-agent`

Use current authoritative Pump docs/IDLs and real mainnet data. Third-party sources may only generate hypotheses.

## Already established — do not reopen unnecessarily

### pump.fun
- TradeEvent reserves are post-trade.
- Account-grounded pre-trade reconstruction works.
- SELL formula is exact on the account-grounded sample.
- BUY is instruction-specific.
- Exact-in BUY shows the `input - 1` rule.
- Token-target BUY is an inverse/token-target calculation.
- Fees are not simply netted from the curve input as M2.1 modeled.

### PumpSwap
- Event reserves are pre-trade.
- Pool reserve fields match pool token-account pre-transaction balances in tested samples.
- Effective quote reserve is `raw_quote + signed virtual_quote_reserves`.
- Pool layouts are versioned by account length.
- SELL `(2,93)` and `(25,5)` are exact.
- `(20,5)` SELL remains unresolved.
- `(25,5)` BUY is mostly exact.
- `(2,93)` and `(20,5)` BUY remain unresolved.
- Fee state must be per-market and observed, not a caller constant.

## PumpSwap final investigation

### A. `(20,5)` SELL
Start from:
`gross_quote_out = floor(q_eff * base_in / (base_reserve + base_in))`

where:
`q_eff = raw_quote + virtual_quote_reserves`

Investigate the ~920,619 residual found in M2.2A across multiple independent `(20,5)` pools.

Test systematically:
- raw/effective base and quote reserves
- Pool vs event virtual reserves
- appended/version-specific Pool fields
- creator-fee state
- fee amounts and rounding
- instruction arguments
- transaction account ordering
- program/config/version regime
- trade-size dependence of the residual
- any additional virtual-base or reserve adjustment

Target >=20 independent `(20,5)` pools and >=10 consecutive swaps per selected pool where feasible.

### B. PumpSwap BUY
Partition by `(25,5)`, `(2,93)`, `(20,5)`, Pool version/length, and creator-fee state.

Test:
- `user_quote_amount_in`
- `user_quote_amount_in - 1`
- `quote_amount_in`
- `quote_amount_in - lp_fee - protocol_fee`
- fee-adjusted user input
- instruction-specific amounts
- rounding variants
- any version-specific rule

Acceptance is exact integer equality, not ppm closeness.

If regimes genuinely differ, model them as separate supported regimes rather than forcing one formula.

### C. Fee/config/version analysis
Determine whether observed `(2,93)` and `(25,5)` are historical configs, per-pool state, event-calculation values, migration/version artifacts, creator-fee-inclusive values, or another mechanism.

Do not replace observed event fee state with current GlobalConfig merely because current docs say 20/5.

## Pump.fun BUY certification

Use account-grounded pre-trade state.

For each:
- `buy_exact_sol_in`
- `buy_exact_quote_in`
- token-target `buy`

collect a large, diverse sample; target >=1000 per instruction if realistically obtainable.

Verify exact formulas and rounding. Include SOL and non-SOL quote cases where applicable.

Test boundaries:
- 1-lamport input
- 2-lamport input
- dust
- near-reserve
- near-completion
- zero/one output
- overflow boundaries
- real-token-reserve limits

Do not invent the mechanism behind `-1`; distinguish verified behavior from inference.

## Yellowstone observability requirement

For every field needed by the final production quote, prove it is obtainable from Yellowstone-observed account/transaction/event state without RPC polling on the hot path.

Produce:
| Required field | Yellowstone source | Update event | Hot-path required? | RPC required? |

If a required field cannot be obtained deterministically from Yellowstone, mark that regime unsupported unless a safe alternative exists.

## Differential evidence

For every candidate formula record:
- observed
- predicted
- signed error
- absolute/relative error
- exact/non-exact
- regime
- Pool length/version
- fee regime
- signature
- slot

EXACT means integer equality.

Use `u128`/`i128` safely.

Evidence labels:
`VERIFIED`, `OBSERVED`, `INFERRED`, `HYPOTHESIS`, `UNRESOLVED`.

Never promote a hypothesis to VERIFIED because it fits a sample.

## Hard stop

PumpSwap is CLOSED only if reserves, effective reserves, supported fee regimes, SELL, BUY, version differences, and Yellowstone-required state are proven.

pump.fun BUY is CLOSED only if supported instructions have exact formulas, sufficient diverse account-grounded evidence, boundary/rounding tests, and Yellowstone-observable required state.

If a regime cannot be proven, STOP investigating it indefinitely. Instead define an exact deterministic exclusion:
`venue + instruction + version/fee regime + missing state`.

## No production implementation

Do not modify:
- `src/quote.rs`
- `src/market.rs`
- production decode/state logic
- execution logic
- M3 qualification

Research scripts are allowed. Do not weaken existing tests to force success.

## Final report

Write:
`docs/MILESTONE_2_2B_FINAL_PROTOCOL_PARITY_REPORT.md`

Include:
1. Executive conclusion
2. Pump.fun BUY certification
3. PumpSwap `(20,5)` SELL
4. PumpSwap BUY
5. Fee/version analysis
6. Yellowstone observability
7. Exact formula table
8. Exact parity statistics
9. Remaining unresolved items
10. Supported vs unsupported regimes
11. Final production recommendation
12. Representative signatures/slots

End with exactly one:
`PROTOCOL PARITY CLOSED`
or
`PROTOCOL PARITY CLOSED WITH EXCLUSIONS`
or
`PROTOCOL PARITY NOT CLOSED — SPECIFIC BLOCKER REMAINS`

This is the final investigation pass. Do not broaden scope or start another research phase. The output must definitively state what quote math Neurone can trust from Yellowstone state and what it must refuse to trade.
