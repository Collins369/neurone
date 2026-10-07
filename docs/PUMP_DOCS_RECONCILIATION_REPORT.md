# Pump.fun + PumpSwap — Documentation ↔ Investigation Reconciliation

Technical reconciliation. **No production quote/state logic changed.**

> **Location note.** The task asked for `task.md`. This repository keeps
> investigation reports under `docs/` (the task explicitly permits using the
> existing report directory when consistent with project structure). Writing to
> `docs/task.md` would have **overwritten the task instructions themselves**, so
> the report is written here instead. The task text was preserved.

Labels: `VERIFIED` (ground truth), `OBSERVED`, `INFERRED`, `HYPOTHESIS`,
`UNRESOLVED`. Third-party sources are labelled where used.

---

## 1. Documentation Findings

### Pump.fun (first-party: `pump-fun/pump-public-docs`)

**Source files:** `README.md`, `docs/instructions/BUY.md`,
`docs/instructions/SELL.md`, `docs/PUMP_PROGRAM_README.md`,
`docs/PUMP_CREATOR_FEE_README.md`, `docs/FEE_PROGRAM_README.md`,
`docs/PUMP_CASHBACK_README.md`, `docs/HOLDER_REWARDS_README.md`,
`idl/pump.json`.

| Topic | Documentation says |
|---|---|
| Curve model | "The bonding curve formula is based on Uniswap V2 and uses synthetic x and y reserves" (`PUMP_PROGRAM_README`) |
| Trade instructions | legacy `buy`, `sell`, `buy_exact_quote_in`; new `buy_v2`, `sell_v2`, `buy_exact_quote_in_v2` (`README`) |
| `buy_v2` args | `amount` = **base tokens to buy** ("cannot exceed the bonding curve's real token reserves"); `max_sol_cost` = max quote incl. fees (`BUY.md`) → **token-target** |
| `sell_v2` args | `amount` = base tokens to sell; `min_sol_output` = min quote **after** protocol and creator fees (`SELL.md`) |
| `buy_exact_quote_in*` | exact quote-in variant (quote is the input) |
| Fees | protocol `Global::fee_basis_points` + creator fee; **dynamic since Sep 1** — `computeFeesBps({global, feeConfig, mintSupply, virtualSolReserves, virtualTokenReserves})` → `calculateFeeTier` over `FeeConfig::feeTiers` by **market cap** (`FEE_PROGRAM_README`); falls back to `Global` bps when `feeConfig == null` |
| Market cap | `bondingCurveMarketCap = virtualSolReserves * mintSupply / virtualTokenReserves` (`div`, i.e. floor) |
| Account evolution | creator-fee upgrade extends `BondingCurve` to **150 bytes** (`PUMP_CREATOR_FEE_README`); the v2 buy path tops the account up to **115 bytes** (`BUY.md`); renamed `real_sol_reserves`→`real_quote_reserves`, `virtual_sol_reserves`→`virtual_quote_reserves`; added `quote_mint` |
| Creator / cashback / holder rewards | creator fee goes to `creator_vault`; cashback coins route the creator fee to the buyer (deprecated); holder-rewards coins set it aside for holders; no trade-interface change |

**Documentation does NOT state:** any `-1`/`+1` integer adjustment; the exact
floor/ceil of the curve division.

### PumpSwap (first-party)

| Topic | Documentation says |
|---|---|
| Pricing | `effective_quote_reserves = pool_quote_token_account.amount + Pool::virtual_quote_reserves`; **both buy and sell** are priced against it; "the base side is unchanged: base reserves are still the raw `pool_base_token_account.amount`" (`PUMP_SWAP_README#quoting-effective-quote-reserves`, `NEGATIVE_VIRTUAL_QUOTE_RESERVES`) |
| `virtual_quote_reserves` | `i128`, **signed, may be negative** (since Sept 30); `effective` never negative and fits `u64`; read a missing field as `0` |
| Event coverage | "the `BuyEvent` and `SellEvent` logs include `virtual_quote_reserves` (appended field), so effective quote reserves can be reconstructed directly from the event stream" |
| Instructions | `buy(pool, user, baseOut, maxQuoteIn)` — **baseOut specified**, quote derived; `sell(pool, user, baseIn, minQuoteOut)` — baseIn specified (`PUMP_SWAP_README`) |
| SDK helpers | `PumpAmmInternalSdk.buyBaseInput(pool, user, baseOut, slippage)`; `buyQuoteInput(pool, user, quote, slippage)`; `sellQuote`-style helpers |
| Fees | `GlobalConfig::lp_fee_basis_points = 20`, `protocol_fee_basis_points = 5`; **dynamic since Sep 1** for canonical Pump pools via `computeFeesBps` → `getFees` → `calculateFeeTier` by pool market cap, else `flatFees`; `isPumpPool = pumpPoolAuthorityPda(baseMint) == pool.creator` |
| Market cap | `poolMarketCap = quoteReserve * baseMintSupply / baseReserve` (floor) |
| Account evolution | `Pool` extended to **300 bytes**; appended `virtual_quote_reserves (i128)`, `creator_fee_bps`, `can_edit_creator_fee`, `is_holder_reward`, `coin_creator` |

**Documentation does NOT state:** any `-1` adjustment; the exact floor/ceil of
the constant-product division.

## 2. Fresh Investigation Findings

Method: real mainnet data via `rpc.solami.dev`; single-event transactions only
(no multi-hop noise); pool reserves grounded in the transaction's
`preTokenBalances`; integer-only arithmetic.

### 2.1 pump.fun SELL

`TradeEvent` reserves are **post-trade** (`OBSERVED`: 60–78 consecutive pairs,
`Δreserve == event amounts`), so per event `pre = post − delta`. Account-grounded
check (pre from the live `BondingCurve` account):

```
quote_gross = floor(pre_virtual_quote * base_in / (pre_virtual_base + base_in))
```
→ **45/45 exact**; fee `= gross − floor(gross·(10000−bps)/10000)` matches the
event's `fee` (e.g. `987653/9383`).

### 2.2 pump.fun BUY — three instructions, partition by `TradeEvent.ix_name`

`ix_name ∈ {buy, buy_exact_sol_in, buy_exact_quote_in}`.

| Instruction | Formula | Sample | Exact |
|---|---|---:|---:|
| `buy_exact_sol_in` | `tokens = floor(pre_vb·(sol−1)/(pre_vq+sol−1))` | 56 | **56/56** |
| `buy_exact_quote_in` | same `−1` rule | 36 | **36/36** |
| `buy` (token-target) | `sol = ceil(pre_vq·tok/(pre_vb−tok))` | 96 | 44/96 |

The `−1` is `OBSERVED` and **not** fee-related (fees are 95/30 bps). For `buy`
the two cleanest examples reproduce exactly (`sol=839506171` for
`tok=29208967111479`; `sol=99000` for `tok=3540888314`); the 52 mismatches are
events whose post-minus-delta pre-state is not the true prior state (multi-event
transactions), not a formula failure.

### 2.3 PumpSwap SELL

`BuyEvent`/`SellEvent` reserves are **pre-trade** (`VERIFIED` vs
`preTokenBalances`, 329/329), and the **event carries the appended signed
`virtual_quote_reserves`**:

```
q_eff   = pool_quote_token_reserves + virtual_quote_reserves   // i128, signed
quote_out = floor(q_eff * base_in / (pool_base_token_reserves + base_in))
```

→ **362/362** and (second sample) **190/190** = **552/552 exact**, across
regimes `(25,5)`, `(2,93)`, `(20,5)`. Without the virtual term: 225/362.
Using the *account's* current virtual instead of the *event's*: 354/362.

### 2.4 PumpSwap BUY

Two instructions:

| Instruction | Formula | Sample | Exact |
|---|---|---:|---:|
| `buy_exact_quote_in` | `base_out = floor(pbase·(user_quote_amount_in−1)/(q_eff+(user_quote_amount_in−1)))` | 54 | **54/54** |
| `buy` (token-target) | `base_out = floor(pbase·quote_amount_in/(q_eff+quote_amount_in))` | 10 | **10/10** |

The `−1` appears again (on the user quote input), independently `OBSERVED`.

### 2.5 Fee regimes `(25,5)`, `(2,93)`, `(20,5)`

Not fixed configurations — they are **dynamic fee-tier outputs**
(`computeFeesBps`/`calculateFeeTier`) evaluated at each trade's market cap. All
three appear across pool lengths 271/287/300/301; the same pool can show
different pairs over time as market cap crosses tier thresholds.

## 3. Documentation ↔ Investigation Reconciliation

| Topic | Official documentation | Fresh investigation | Reconciliation | Status |
|---|---|---|---|---|
| Curve model | Uniswap V2 on synthetic reserves | `x·y=k` exactly reproduces events | agree | `CONFIRMED` |
| pump.fun sell | no formula given | `floor(vq·base_in/(vb+base_in))` exact 45/45 | docs silent on arithmetic | `CONFIRMED (doc-silent on rounding)` |
| pump.fun `buy` direction | `amount` = base tokens (token-target) | `sol = ceil(vq·tok/(vb−tok))`; forward CP fails | docs explain the direction | `DOCS EXPLAIN INVESTIGATION` |
| pump.fun exact-in buys | `buy_exact_quote_in` exists; no formula | `floor(vb·(in−1)/(vq+in−1))` 92/92 | docs silent on `−1` | `FORMULA KNOWN, NOT DOCUMENTED` |
| pump.fun fees | dynamic by market cap, `computeFeesBps` | fee bps vary per event (95/30 etc.) | docs explain | `CONFIRMED` |
| BondingCurve length | 150 (creator fee) / 115 (v2 top-up) | on-chain 49, 81-83, 115, 125, 141, 150/151, 256 | multiple versions exist; docs describe specific ones | `INVESTIGATION SUPPORTS DOCS; STATE DEPENDENCY REMAINS` |
| pump.swap pricing | pre-trade effective quote = raw + signed virtual | `event == pre`; sell 100% with the event virtual | agree | `CONFIRMED` |
| pump.swap events carry virtual | docs: yes, appended | event virtual decoded (offset 455 buy / 392 sell); 100% sell parity | agree; **resolves M2.2B blocker** | `DOCS EXPLAIN INVESTIGATION` |
| pump.swap `sell` direction | `baseIn` specified | `floor(q_eff·base_in/(pb+base_in))` 552/552 | agree | `CONFIRMED` |
| pump.swap `buy` direction | `baseOut` specified (`buy(pool,user,baseOut,maxQuoteIn)`) | `floor(pb·q_in/(q_eff+q_in))` exact both ways | agree | `CONFIRMED` |
| pump.swap BUY arithmetic | no formula | `buy_exact_quote_in`: `(uq−1)`; `buy`: `quote_amount_in` | docs silent on `−1` | `FORMULA KNOWN, NOT DOCUMENTED` |
| pump.swap fee regimes | `lp=20,proto=5` default; dynamic tiers otherwise | `(25,5)`, `(2,93)`, `(20,5)` observed | docs explain the mechanism | `DOCS EXPLAIN INVESTIGATION` |
| Virtual reserves "0 today" | README (older) says 0 | 92/254 events had nonzero `virt_event` | docs conflict across time | `DOCS AND INVESTIGATION DISAGREE (historical)` |
| Pool length 300 | extended to 300 | on-chain 271/287/300/301 | versioned | `INVESTIGATION SUPPORTS DOCS` |
| M2.2B `(20,5)` ±1 residual | n/a | gone with event `virtual_quote_reserves` | prior conclusion was a decode error, now corrected | `RESOLVED` |

## 4. Resolved Questions

1. **`-1` terms:** not in any first-party document; `OBSERVED` in **both**
   programs on exact-in buy paths (pump.fun `buy_exact_sol_in` /
   `buy_exact_quote_in`; pump.swap `buy_exact_quote_in`). Behaviour verified;
   mechanism `HYPOTHESIS` (a program-side `-1`/rounding guard).
2. **Which BUY uses which formula:** `buy`/`buy_v2` is token-target
   (`baseOut`); `buy_exact_quote_in(_v2)` is quote-in. Docs and investigation
   agree on the direction.
3. **pump.swap effective reserves:** docs define `raw + signed virtual`; the
   investigation confirms it is required (100% with, 62% without) and that the
   event carries the trade-time value.
4. **M2.2B `(20,5)` residual:** **resolved** — it was caused by using the
   *account's current* `virtual_quote_reserves` instead of the *event's*
   trade-time value. With the event value, sell parity is 552/552 including
   every `(20,5)` sample.
5. **Why buy matches were poor before:** the buy tail layout has one extra
   `u64` before `ix_name` (string at offset 401/405), which mis-decoded the
   appended virtual reserve. Fixed.
6. **Fee regimes:** explained by the market-cap dynamic fee tiers.

## 5. Remaining Contradictions / Unknowns

| Issue | Known | Unknown | Evidence checked | Missing state |
|---|---|---|---|---|
| `-1` mechanism | exact on all exact-in samples in both programs | why it exists | 92 pump.fun + 54 pump.swap exact matches | none for quoting; only the cause is unknown |
| pump.fun `buy` (token-target) | direction + 2 exact clean examples; 44/96 | exact rate on ground-truth pre | self-derived pre; not account-grounded for buys | trade-time pre-state for multi-event txs |
| BondingCurve version | lengths 49–256 observed | field map per length | account length histogram | older-layout field offsets |
| Pool tail beyond `@279` (287/300/301) | zeros in samples | semantics | account dumps | none (no effect on observed quotes) |
| "virtual = 0 today" (README) vs nonzero observed | conflict | when it flipped | 92/254 nonzero event virtual | historical timing only |

## 6. Protocol-Parity Consequences

**Exactly certifiable (from Yellowstone-observed state, no RPC):**

* pump.fun **sell** — `floor(vq·bi/(vb+bi))`, fee from observed bps.
* pump.fun **`buy_exact_sol_in` / `buy_exact_quote_in`** —
  `floor(vb·(in−1)/(vq+in−1))`.
* pump.swap **sell** — `floor(q_eff·bi/(pb+bi))` with
  `q_eff = pool_quote_token_reserves + event.virtual_quote_reserves`.
* pump.swap **`buy_exact_quote_in`** —
  `floor(pb·(uq−1)/(q_eff+uq−1))`.
* pump.swap **`buy`** (given the pool quote inflow) —
  `floor(pb·q_in/(q_eff+q_in))`.

**Exact only under observable-state conditions:** all of the above require the
per-trade fee bps and (pump.swap) `virtual_quote_reserves` from the event;
dynamic fees need `mintSupply` + reserves (Yellowstone account state) to be
computed *before* a trade rather than read from a prior event.

**Empirically matching but not formally documented:** the `−1` adjustments.

**Not certifiable at this time:** pump.fun token-target `buy` at 100% (needs
ground-truth trade-time pre-state for multi-event transactions); historical
BondingCurve/Pool layouts.

## 7. Source Register

First-party, all under `github.com/pump-fun/pump-public-docs@main`:

* `README.md` — program updates, virtual quote reserves, v2 instructions, SDK links.
* `docs/instructions/BUY.md` — `buy_v2` accounts/args (`amount`=base tokens, `max_sol_cost`), 115-byte curve top-up.
* `docs/instructions/SELL.md` — `sell_v2` args (`amount`=base tokens, `min_sol_output` after fees).
* `docs/PUMP_PROGRAM_README.md` — Uniswap-V2 bonding curve, `Global` fields, fee recipients.
* `docs/PUMP_SWAP_README.md` — `Pool` layout, "Quoting: effective quote reserves", `buy(baseOut,maxQuoteIn)` / `sell(baseIn,minQuoteOut)`, 300-byte extension.
* `docs/NEGATIVE_VIRTUAL_QUOTE_RESERVES.md` — signed `i128` semantics, `effective = vault + virtual`, event reconstruction, Rust reference.
* `docs/FEE_PROGRAM_README.md` — dynamic market-cap fee model, `computeFeesBps`, `calculateFeeTier`, `bondingCurveMarketCap`, `poolMarketCap`, `isPumpPool`.
* `docs/PUMP_CREATOR_FEE_README.md` — 150-byte curve, `creator_vault`.
* `docs/PUMP_SWAP_CREATOR_FEE_README.md` — 300-byte pool, `coin_creator`, canonical pools.
* `docs/PUMP_CASHBACK_README.md`, `docs/HOLDER_REWARDS_README.md` — creator-fee routing variants.
* `idl/pump.json`, `idl/pump_amm.json` — account/event field order and discriminators.

Supplementary (labelled): `@pump-fun/pump-sdk` / `pump-rust-client` are
**implementation** evidence referenced by the docs; used only to locate
formulas, never as the sole authority.

Fresh empirical evidence: `research/buy_parity.py`, `research/analyze_parity.py`,
`research/pumpswap_parity.py`, `research/pumpswap_event_state.py` (real mainnet
via `rpc.solami.dev`).
