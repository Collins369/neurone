# Neurone — M3.2: Pump.fun SELL + Token-Target BUY Parity Investigation

Investigation only. No formula or production change was made.

Statuses: `CONFIRMED` (official source / ground truth), `OBSERVED`,
`INFERRED`, `HYPOTHESIS`, `UNRESOLVED`.

---

## 1. Documentation Findings

**Primary new source (first-party, decisive):** the official TypeScript SDK
`@pump-fun/pump-sdk` **v3.0.0** (`package/src/bondingCurve.ts`, `fees.ts`),
which the pump docs explicitly point to as the reference implementation of the
quote math (`FEE_PROGRAM_README.md`: "The new fee structure code is present in
… our Typescript SDKs").

### Exact integer quote math (`CONFIRMED`)

Constant-product helpers (BN `.div` truncates toward zero = floor, inputs
non-negative):

```
buyTokensFromSol(in)   = in * virtualTokenReserves / (virtualQuoteReserves + in)
sellSolFromTokens(in)  = in * virtualQuoteReserves / (virtualTokenReserves + in)
solForTargetTokens(x)  = x * virtualQuoteReserves / (virtualTokenReserves - x) + 1
```

**SELL** — `getSellSolAmountFromTokenAmount(amount)`:

```
solCost = sellSolFromTokens(amount)              // floor
return  solCost - getFee(amount = solCost)       // user receives the NET
```

**BUY (SOL in)** — `getBuyTokenAmountFromSolAmount(amount)`:

```
totalFeeBps = protocolFeeBps + (creator != default ? creatorFeeBps : 0)
inputAmount = (amount - 1) * 10000 / (totalFeeBps + 10000)      // floor
tokens      = buyTokensFromSol(inputAmount)                      // floor
return min(tokens, realTokenReserves)
```

**Token-target BUY** — `getBuySolAmountFromTokenAmount(amount)`:

```
minAmount = min(amount, realTokenReserves)
solCost   = solForTargetTokens(minAmount)        // .../() + 1
return  solCost + getFee(amount = solCost)       // user pays the GROSS
```

These pin three things the previous milestones only inferred:

1. **The `−1` is officially documented** (`amount.subn(1)`) — `CONFIRMED`.
2. **Fee application is an inversion, not a subtraction:**
   `input = (amount−1)·10000/(feeBps+10000)`, i.e. `amount` is gross-of-fees and
   the curve receives the net after fee *bumped* back up. Neurone used
   `net = input − floor(input·fee/10000)`, which is a different integer result.
3. **The creator fee applies only when `bondingCurve.creator != Pubkey::default`**
   (`isNewBondingCurve || !default.equals(creator)`), and token-target uses
   `+1` (not `ceil`).

Fees themselves are dynamic: `computeFeesBps` selects a `FeeConfig` tier by
**market cap** (`bondingCurveMarketCap = vQuote·supply/vToken`), else the
`Global` bps. Quote mints are classified SOL-like / stable (USDC) / exotic
(`fees.ts`, `initialVirtualQuoteReservesFor`). The docs also define v3 helpers
`getBuyV3QuoteAmountFromTokenAmount` / `getBuyV3TokenAmountFromQuoteAmount`.

**Docs silent on:** why `Event.post − delta` would ever differ from the true
pre-state (no explicit statement about buyback / holder-reward / cashback
instructions mutating the curve without a `TradeEvent`).

## 2. Dataset / Sample Selection

* Live Yellowstone (M3/M3.1 harness) plus cached samples collected with
  `rpc.solami.dev` (271 SELL, 96 `buy`, 56 `buy_exact_sol_in`, 36
  `buy_exact_quote_in`, plus USDC pairs).
* Contiguity filter: consecutive same-curve events where
  `Δreserve == event amounts` (proves the event's reserves are post-trade and
  the predecessor is the true pre-state).
* Instruction partition by `TradeEvent.ix_name`.

## 3. Pump.fun SELL Investigation

Using ground-truth contiguous pairs (predecessor post-state = pre-state):

```
gross = floor(vQuote_pre * base_in / (vToken_pre + base_in))
```

* `gross == event.sol_amount` for **219/271** (~81%) — the event's `sol_amount`
  is the curve's **gross** quote delta (`OBSERVED`).
* Alternative interpretations fail: `gross − fee == sol_amount` (0/271),
  `gross == sol_amount + fee + creator` (5/271).

So the formula matches the official SDK helper and the event semantics for the
majority; the residual ~19% are events whose `post − delta` is **not** the true
pre-state.

## 4. Pump.fun Token-Target BUY Investigation

Official formula (`solForTargetTokens + fee`, `+1`) matches the two clean live
examples exactly (e.g. `target = 29,208,967,111,479 → 839,506,171`). Live
parity is 71% on the event-derived pre-state; the shortfall has the same
signature as SELL (pre-state, not formula).

## 5. Exact vs Failing Event Comparison

| Field | Exact SELL sample | Failing SELL sample | Interpretation |
|---|---:|---:|---|
| vToken post | 1,075,682,474,849,693 | 1,067,547,944,313,663 | both plausible |
| vQuote post | 2,164,115,499 | 13,226,060,869 | — |
| real token post | 795,782,474,849,693 | 787,647,944,313,663 | `vToken − real = 279.9e12` in both |
| token_amount | 491,141,116,763 | 2,370,898,772,047 | — |
| sol_amount | 987,653 | 43,446,098 | — |
| CP(pre=post−delta) | **= 987,653** | **29.5M ≠ 43.4M** | pre-state differs |

Both carry the same layout (virtual−real = the fixed 279.9e12 constant), the
same discriminator, and a recognized `ix_name`. Nothing in the *fields* differs;
only the arithmetic relation fails, which points at the **state**, not the
decode or the formula.

## 6. TradeEvent Semantics

`CONFIRMED`: reserves are **post-trade** (contiguous-pair equality, and the
official SDK prices from the account's current (post) reserves). `sol_amount`
is the curve's gross quote delta on SELL and gross input on BUY. The appended
`quote_amount`/`virtual_quote_reserves`/`real_quote_reserves` carry the non-SOL
(USDC) pair values (M3.1 fix).

## 7. Transaction / Instruction Ordering

`HYPOTHESIS`: the failing events are trades whose curve was mutated **between
the trade and the snapshot the event reports** — e.g. a same-transaction
`buyback_buy_and_burn`, holder-reward/cashback accounting, or an
`extend_account`/`set_virtual_quote_reserves` step — which changes reserves
without emitting a `TradeEvent`. Then `event.post − delta` no longer equals the
trade's pre-state. This is consistent with: layout-consistent fields, formula
correct on the majority, and the account stream failing to corroborate the
pre-state in ~53% of correlated trades.

`UNRESOLVED`: not yet proven per-transaction because the failing events were
not retained with their transaction instruction logs.

## 8. Account-State Correlation

M3.1 added a narrow pump.fun bonding-curve account subscription
(`owner = pump.fun` + BondingCurve memcmp) and a slot-versioned cache. Result:
**1,306/2,762 correlated trades corroborated (47%)**; the account stream lags
the transaction stream, so it validates roughly half and does not repair the
residual in a single pass.

## 9. Program Variants

* **Non-SOL (USDC) quote pairs — `CONFIRMED`, fixed in M3.1.** `sol_amount = 0`;
  values live in the appended quote fields.
* **Mayhem / cashback / holder-reward / buyback — `HYPOTHESIS`.** Officially
  documented variants; the buyback/holder-reward paths can move reserves outside
  a `TradeEvent`, which is the leading candidate for the residual.
* **Dynamic fee tiers** (`(25,5)`, `(2,93)`, `(20,5)`) — `CONFIRMED` as
  market-cap tiers, not variants.

## 10. Formula Reconciliation

| | Formula |
|---|---|
| **A — Neurone (M3.1)** | CP helpers identical; BUY `net = in − floor(in·fee/10000)`; SELL `gross`; token-target `ceil` |
| **B — official docs** | reference the SDK; no closed-form in the markdown |
| **C — official SDK v3.0.0** | CP helpers identical; BUY `in = (amount−1)·10000/(feeBps+10000)`; SELL `gross − fee`; token-target `x·vq/(vt−x)+1` then `+ fee`; creator fee gated on `creator != default` |
| **D — on-chain ground truth** | matches C on the contiguous/account-grounded samples |

`A` matches `C` on the constant-product core (so the SELL mismatches are not a
formula error), but `A` **differs from `C` on fee application and on the
creator-fee gate**, which biases BUY and SELL outputs by the fee rounding
difference for a subset.

## 11. Root Cause

1. **Fee handling mismatch (`CONFIRMED` as a defect).** Neurone subtracts a
   floored fee; the protocol/SDK invert it. This alone changes results wherever
   a fee applies.
2. **Pre-state availability (`UNRESOLVED`, leading `HYPOTHESIS`).** For ~19–29%
   of SELL/token-target events, `event.post − delta` is not the trade's true
   pre-state, most plausibly because a same-transaction instruction mutated the
   curve outside a `TradeEvent`.

## 12. Minimum Correct Fix

1. Adopt the official SDK integer sequence for the fee path:
   * BUY: `in = (amount−1)·10000/(protocolBps + creatorBps·[creator != default] + 10000)`.
   * SELL: `out = floor(vq·tok/(vt+tok)) − fee(out)`.
   * token-target: `cost = x·vq/(vt−x) + 1` then `+ fee(cost)`.
2. Source fee bps from the observed trade event (`fee_basis_points`,
   `creator_fee_basis_points`) or `computeFeesBps` at the market cap shown by
   the trading pair — never a caller constant.
3. Gate the residual: require the pre-state to be corroborated (the previous
   event's post-state or the account stream at the trade slot) and otherwise
   mark the trade **not reconstructable** (`UnsupportedState`) rather than
   quoting from a possibly-mutated snapshot.
4. `decode`: keep the USDC quote-field fix and the program-attribution fix.

This is a decoder/fee-application change plus a state gate — no architecture
change, no RPC in the hot path.

## 13. Yellowstone-Only Feasibility

* PumpSwap (all paths) and pump.fun exact-in buys: `CONFIRMED` Yellowstone-only.
* pump.fun SELL / token-target BUY: **feasible in principle** — the fee fix
  removes one error class; the residual needs the *transaction-local* curve
  state (the instruction logs already streamed) to detect intermediate curve
  mutations. If a mutation emits no event and no account write is observed
  before the trade, that trade is **not reconstructable from Yellowstone alone**
  and must be excluded.

## 14. Remaining Unknowns

* Whether the failing events are caused by buyback/holder-reward/cashback
  instructions, same-slot ordering, or another unmodeled mutation (`HYPOTHESIS`).
* Whether the fee-application fix alone raises SELL/token-target to 100%
  (needs a live re-run after implementing the SDK sequence).

## 15. Evidence Register

**First-party:** `pump-fun/pump-public-docs@main` — `README.md`,
`docs/instructions/BUY.md`, `docs/instructions/SELL.md`,
`docs/PUMP_PROGRAM_README.md`, `docs/PUMP_SWAP_README.md`,
`docs/FEE_PROGRAM_README.md`, `docs/NEGATIVE_VIRTUAL_QUOTE_RESERVES.md`,
`docs/PUMP_CREATOR_FEE_README.md`, `docs/PUMP_CASHBACK_README.md`,
`docs/HOLDER_REWARDS_README.md`, `idl/pump.json`.
Official SDK `@pump-fun/pump-sdk@3.0.0` — `src/bondingCurve.ts`
(`getBuyTokenAmountFromSolAmount`, `getBuySolAmountFromTokenAmount`,
`getSellSolAmountFromTokenAmount`, the three `…Quote` helpers, `getFee`,
`computeFeesBps`, `calculateFeeTier`), `src/fees.ts`.

**Empirical:** M3/M3.1 live runs and cached samples via `rpc.solami.dev`;
`research/*` harnesses.

**Status summary:** official formulas `CONFIRMED`; the `−1` `CONFIRMED`;
fee-application mismatch `CONFIRMED`; the SELL/token-target residual mechanism
`UNRESOLVED` (leading `HYPOTHESIS`: intermediate curve mutation in the same
transaction).

**Minimum next action:** implement the official SDK fee sequence and re-run the
live parity harness with a pre-state corroboration gate; if the residual
persists, capture the failing events with their full instruction logs to prove
the mutation hypothesis.
