# Neurone — M2.2A Protocol Parity Closure Report

**Investigation only. No production quote/state changes were made.**
`NEURONE_BLUEPRINT.md` and historical reports untouched.

Labels: `VERIFIED` (ground-truth data), `OBSERVED` (measured), `INFERRED`,
`HYPOTHESIS`, `UNRESOLVED`.

Tooling: `research/buy_parity.py`, `research/analyze_parity.py`,
`research/pumpswap_parity.py`.

---

## 1. Executive conclusion

* **pump.fun SELL — `VERIFIED` exact.** With pre-trade reserves reconstructed
  from the on-chain `BondingCurve` account (ground truth, not a neighbour
  event), `quote_gross = floor(v_quote·base_in / (v_base + base_in))` is exact
  **45/45**. `TradeEvent`s store **post-trade** reserves (78/78 consecutive
  pairs).
* **pump.fun BUY — `OBSERVED` + `INFERRED`, not certified.** There are three
  instructions (`buy_exact_sol_in`, `buy_exact_quote_in`, `buy`); the first two
  match `floor(v_base·(in−1)/(v_quote+in−1))` and `buy` is token-target
  (`in = ceil(v_quote·tok/(v_base−tok))`). Exact on the samples gathered
  (31/37, 29/32, 50/54) but the per-instruction labelled population is below
  1,000 and not 100%, so parity is **not certified**.
* **pump.swap — `UNRESOLVED`.** Reserves are **pre-trade** and equal the pool
  token-account balances (329/329); the effective quote reserve is
  `pool_quote_token_reserves + virtual_quote_reserves` (signed). SELL is exact
  for the `(lp=2,proto=93)` and `(lp=25,proto=5)` fee regimes (111/111, 131/131)
  but not for `(lp=20,proto=5)`. BUY is exact for one regime (51/53) and
  unresolved for the others. Pool accounts come in many lengths (211–301).
* **Fee state:** the M2.2 proposal is **confirmed** — authoritative quotes need
  observed `lp_bps`, `protocol_bps`, `creator_bps` + provenance per market.
* **Recommendation:** pump.fun is ready to fix (pending a larger account-grounded
  sample); pump.swap needs more work. Overall: **M2 quote correctness is
  `BLOCKED`** (not closed).

## 2. pump.fun confirmation

**Reserve semantics — `VERIFIED`.** 78/78 consecutive same-curve event pairs
show `Δreserve == second event's amounts`, i.e. each `TradeEvent` carries the
state **after** its trade. Pre-trade reserves for trade *N* are event *N−1*'s
reserves, **or** the current `BondingCurve` account minus the trade's delta when
the event is the latest on that curve (the account-grounded method below).

**SELL — `VERIFIED` exact.** Using account-grounded pre-state:
`gross = floor(v_quote·base_in/(v_base+base_in))` → **45/45**. Predecessor-based
reconstruction gave only 186/259 because sparse sampling skips intermediate
trades (a reconstruction artifact, not a formula error).

**BUY — `OBSERVED`, per instruction.** `TradeEvent.ix_name` partitions:

| ix_name | samples (events) | formula | exact (predecessor-grounded) |
|---|---:|---|---:|
| `sell` | 273 | `floor(vq·tok/(vb+tok))` | 186/259 pairs |
| `buy_exact_sol_in` | 56 | `floor(vb·(sol−1)/(vq+sol−1))` | 31/37 |
| `buy_exact_quote_in` | 37 | same `−1` rule | 29/32 |
| `buy` (token-target) | 96 | `sol = ceil(vq·tok/(vb−tok))` | 50/54 |

The `−1` input reduction is `OBSERVED` (also 1/1 with account-grounded
pre-state) and is **not** fee-related (`fee_bps` is 95/30 bps, far above 1
lamport). `gross = floor(vb·sol/(vq+sol))` matches 0/37 `buy_exact_sol_in` and
51/78 `buy`, so M2.1's single formula is wrong for two of the three
instructions.

**Boundary tests — `OBSERVED`.** Small (1-lamport) and dust buys still obey the
`−1` rule. `real_token_reserves` never bounded an observed output (it is far
above the emitted amount), and no observed buy hit the curve-completion cap.

**Sample shortfall (`UNRESOLVED`).** Collected: 462 events (273 sells / 96
`buy` / 56 `buy_exact_sol_in` / 37 `buy_exact_quote_in`), plus 46
account-grounded samples (45 sells, 1 buy). **1,000 per instruction was not
reachable**: Solami RPC delivered ~5–10 txs/s and `buy_exact_quote_in` is ~8% of
events, so 1,000 of it needs ≳10,000 trade events (~20,000 transactions,
hours). It is available in principle, not within this session's budget.

## 3. PumpSwap Pool layout / version — `VERIFIED`

The earlier "260-byte Pool" was a **`dataSlice` artifact** of the research
script. Real lengths (n=380): `211, 243, 244, 245, 261, 271, 287, 300, 301`.
The published IDL layout is **271 bytes** and matches the `271`-byte accounts
exactly; `300/301`-byte accounts append ~30 bytes (zero-filled in every
sample). Version by length:

| length | published layout? | notes |
|---|---|---|
| 211 | no (older) | not needed for quoting |
| 243–245 | older | ends at `is_cashback_coin` |
| 261 | older + `virtual_quote_reserves` | |
| 271 | **current IDL** | `…creator_fee_bps, can_edit_creator_fee, is_holder_reward` |
| 287 / 300 / 301 | newer | +30 tail bytes (zeros) |

Offsets used: disc(8) `pool_bump`(1) `index`(2) `creator`(32) `base_mint`(32)
`quote_mint`(32) `lp_mint`(32) `pool_base_token_account`(32)
`pool_quote_token_account`(32) `lp_supply`(8) `coin_creator`(32)
`is_mayhem`(1) `is_cashback`(1) `virtual_quote_reserves`(i128@245)
`creator_fee_bps`(u64). Neurone needs **version-aware decoding** (length gate).

## 4. PumpSwap reserve semantics — `VERIFIED`

Ground truth via the transaction's `preTokenBalances`/`postTokenBalances` for the
pool token accounts: the event's `pool_base_token_reserves` /
`pool_quote_token_reserves` equal the **pre-transaction balances** —
`event == pre` for **329/329** single-event swaps. pump.swap events are
**pre-trade** (the opposite of pump.fun).

## 5. PumpSwap virtual reserves — `VERIFIED`

Effective quote reserve = `pool_quote_token_reserves + virtual_quote_reserves`,
the latter a signed `i128` at offset 245 (negative values are legal). With this,
SELL is exact for two fee regimes (§7). Pools with `virtual_quote_reserves = 0`
are unaffected.

## 6. PumpSwap fee regimes — `OBSERVED`

`(lp_bps, protocol_bps)` observed: `(25,5)` ×184, `(2,93)` ×132, `(20,5)` ×13.
The documented `20/5` is **not universal**. Authority for a quote is the fee
fields carried by the swap event that produced the current reserves.

## 7. PumpSwap SELL parity

| fee regime | samples | formula (effective quote) | exact |
|---|---:|---|---:|
| `(2,93)` | 111 | `floor(q_eff·base_in/(base+base_in))` | **111/111** |
| `(25,5)` | 131 | same | **131/131** |
| `(20,5)` | 8 | same | 0/8 (rel. err ≈ −1.05e-5) |

Overall 242/250 exact. The `(20,5)` pool (`14LR8AFP…`, len 301) has
`virtual_quote_reserves = 17,583,583,979` yet the implied effective reserve is
`87,718,577,273` vs `raw+virt = 87,717,656,654` — a residual of `920,619`,
`UNRESOLVED` (possibly a virtual-**base** term). M2.1's "exact SELL" claim is
therefore **not general** and must be rescinded.

## 8. PumpSwap BUY parity

Single-event transactions, effective quote reserve:

| input amount | exact | regime breakdown |
|---|---:|---|
| `user_quote_amount_in` | 51/79 | 51/53 `(25,5)`; 0 elsewhere |
| `user_quote_amount_in − 1` | 51/79 | 15/21 `(2,93)` |
| `quote_amount_in` | 8/79 | — |
| `quote_amount_in − lp_fee − protocol_fee` | 50/79 | 50/53 `(25,5)` |

`UNRESOLVED`: the buy input source differs by regime; `(2,93)` and `(20,5)` are
not exact. **pump.fun BUY math does not transfer to pump.swap.**

## 9. Candidate formulas and rejection reasons

* pump.fun `gross_cp` for buys — rejected (0/37 exact-in; the curve uses a
  `−1`-adjusted input, and `buy` is token-target).
* pump.fun fee-before-CP — rejected (35/54; buys do **not** net fees out of the
  curve input; fees are reported separately).
* pump.swap SELL on raw reserves — rejected (156/250); effective reserves are
  required.
* pump.swap SELL on effective reserves, `(20,5)` regime — rejected (0/8); a term
  is missing.
* pump.swap BUY with `quote_amount_in`/`q_with` — rejected (≈8/79, 6/79); the
  net user amount is the input.
* `<1 ppm` candidates — not accepted as parity (exactness is the bar).

## 10. Differential parity statistics

| Venue | Side / instruction | Sample | Exact | Max rel. err |
|---|---|---:|---:|---:|
| pump.fun | sell (account-grounded) | 45 | **45 (100%)** | 0 |
| pump.fun | sell (predecessor) | 259 | 186 (72%) | reconstruction |
| pump.fun | `buy_exact_sol_in` | 37 | 31 (84%) | — |
| pump.fun | `buy_exact_quote_in` | 32 | 29 (91%) | — |
| pump.fun | `buy` (token-target) | 54 | 50 (93%) | — |
| pump.swap | sell `(2,93)` | 111 | **111 (100%)** | 0 |
| pump.swap | sell `(25,5)` | 131 | **131 (100%)** | 0 |
| pump.swap | sell `(20,5)` | 8 | 0 | 1.05e-5 |
| pump.swap | buy `(25,5)` | 53 | 51 (96%) | — |
| pump.swap | buy `(2,93)` | 21 | 15 (71%) | — |

## 11. Authoritative fee-state model — `CONFIRMED`

A quote is authoritative only when fee state was observed for that market:

```text
lp_bps, protocol_bps, creator_bps   (observed, per market)
fee_slot, fee_signature             (provenance)
```

Source: the swap event's own `lp_fee_basis_points` / `protocol_fee_basis_points`
(and pump.fun's `fee_basis_points` / `creator_fee_basis_points`). Config
accounts (`Global`, `GlobalConfig`) are a secondary/verification source, not
required on the hot path. Fee regimes vary per pool, so values must live on
`MarketState`, never be a caller constant.

## 12. Final quote-state contract

```text
Yellowstone -> Normalizer -> Parallel Shards -> MarketState -> Quote Engine -> Executable?
```

A quote is authoritative only when, per market:

```text
venue
reserves_known && reserve freshness (slot within bound)
effective reserves: base, quote (+ signed virtual quote reserve, pump.swap)
fee state (lp/protocol/creator bps) + provenance
instruction semantics (pump.fun: exact-in vs token-target)
```

Quote stays O(1), integer-only, allocation-free, lock-free, no RPC.

## 13. Exact production changes required (not implemented)

| File | Change |
|---|---|
| `src/decode/pumpfun.rs` | carry `ix_name`; expose pre/post reserve semantics |
| `src/decode/pump_amm.rs` | version-aware Pool decode by account length; carry signed `virtual_quote_reserves` and per-trade fee bps |
| `src/decode/mod.rs` | add `ix_name`/fees/virtual-reserve fields to `DecodedSwap`/`DecodedAccount` |
| `src/market.rs` | `last_ix`; `lp_bps`, `protocol_bps`, `creator_bps`, `fee_slot`, `fee_signature`; effective quote reserve on pump.swap |
| `src/quote.rs` | pump.fun: exact-in (`−1`) vs token-target vs sell; pump.swap: effective reserves + regime-specific input; reject unknown fees/ix |
| `tests/quote_parity.rs` | rescind the pump.swap "exact" claim; scope it to verified regimes |

## 14. Risks / unresolved items

* `UNRESOLVED`: pump.swap `(20,5)` SELL residual (`+920,619` raw) and the
  `(20,5)`/`(2,93)` BUY inputs — likely an additional virtual (base) term
  and/or a version-specific input rule.
* `UNRESOLVED`: pump.fun per-instruction samples remain below 1,000; the `−1`
  mechanism is unexplained (observed, not fee-related).
* `HYPOTHESIS`: a virtual **base** reserve may also exist.
* `UNRESOLVED`: semantics of the 287/300/301-byte tail fields (all zeros in
  samples).
* Non-SOL quote mints (`buy_exact_quote_in`) need dedicated samples (37 here).

## 15. Final recommendation

**pump.fun: `FIX NOW` after a larger account-grounded sample.** The
instruction-aware formulas are established; scale the account-grounded harness
to ≥1,000 buys per instruction and require 100% exact.

**pump.swap: `MORE INVESTIGATION REQUIRED`.** SELL is exact for two fee regimes;
the `(20,5)` regime and BUY remain unresolved. A production fix now would repeat
M2.1's mistake.

**Overall: M2 quote correctness is `BLOCKED`, not closed.** The precise blocker
is the pump.swap `(20,5)` residual term and the regime-specific BUY input, plus
the sample volume needed to certify pump.fun BUY.
