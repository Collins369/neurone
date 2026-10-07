# Neurone Milestone 2.2 — BUY-Quote Parity Investigation

**Investigation only. No production quote change was implemented.**
`NEURONE_BLUEPRINT.md` and historical reports were not modified.

Claim tags used throughout: `VERIFIED` (proven from source/authoritative doc),
`OBSERVED` (measured on real mainnet data), `INFERRED`, `HYPOTHESIS`,
`UNRESOLVED`.

---

## 1. Executive conclusion

* **pump.fun BUY — root cause found (`OBSERVED`).** M2.1 applied a single
  constant-product-on-net-input formula to *three different buy instructions*.
  The `TradeEvent.ix_name` field separates them, and each needs different math:
  * `buy_exact_sol_in` / `buy_exact_quote_in`: `tokens = floor(vtok·(sol−1) /
    (vsol+sol−1))` — **20/20 and 30/32 exact**.
  * `buy`: the protocol is **token-target**; the native direction is
    `sol = ceil(vsol·tok/(vtok−tok))` — **50/54 exact**. A "SOL in → tokens out"
    quote is not the protocol's native direction for this instruction.
  The M2.1 formula matched none of the exact-in samples and ~72% of `buy`.
* **pump.swap — `MORE INVESTIGATION REQUIRED` (`UNRESOLVED`).** M2.1's
  "exact sell parity" was based on 2 samples; over 646 real sells it is exact
  only **405/646 (63%)**. Mismatching pools use a different fee regime
  (`lp_bps=2`, `proto_bps=93` vs the documented `20/5`) and are off by up to a
  factor of ~25, consistent with an unmodeled **virtual-quote-reserve**
  adjustment (official `NEGATIVE_VIRTUAL_QUOTE_RESERVES.md`). The `Pool`
  account size (260 B) also does not match the published IDL layout (~271 B).
* **Fee state.** `fee_bps` must not remain a caller argument: fees vary by
  venue, pool and instruction (95/30 bps seen on pump.fun; 2/93 on some
  pump.swap pools).
* **Recommendation:** `MORE INVESTIGATION REQUIRED` before any production fix
  (driven by pump.swap); the pump.fun fix is ready but must be confirmed on a
  ≥1000-sample set first.

## 2. Current M2.1 behaviour (verification)

`src/quote.rs` computes, for both venues:
`Buy: net = input − input·bps/10000; out = floor(reserve_out·net/(reserve_in+net))`
and `Sell: gross = floor(reserve_out·in/(reserve_in+in)); net = gross − gross·bps/10000`.

Answers to §9:

| # | Question | Answer |
|---|---|---|
| 1 | pump.fun BUY mathematically wrong? | **Yes** (`OBSERVED`) — wrong direction + missing per-instruction handling |
| 2 | Which operation? | instruction semantics (token-target vs exact-in) and the `−1` input adjustment |
| 3 | Fee calculation wrong? | buy fee is *not* subtracted from the curve input on-chain; fee is reported separately |
| 4 | Rounding wrong? | effectively yes: the on-chain buy uses `(sol−1)`, not `sol` |
| 5 | Reserve input wrong? | **Yes** — M2.1 used one formula for 3 instructions |
| 6 | Real-reserve cap missing? | not the cause: `real_token_reserves` ≫ observed outputs in all samples |
| 7 | Is `−1` real/required? | **`OBSERVED`** — exact for 50/52 exact-in buys |
| 8 | Is pump.swap BUY wrong? | **Yes** (`OBSERVED`) — 0/242 exact on the candidate grid |
| 9 | Which operation differs? | reserve semantics + a likely virtual-reserve/fee-regime term |
| 10 | `<1ppm` cause? | mixed instruction types + unmodeled virtual reserves/fee regime |
| 11 | Protocol-version differences? | **Yes** (`OBSERVED`): fee regimes and possibly pool layout vary |
| 12 | Current tests asserting wrong invariant? | yes — `tests/quote_parity.rs` pins pump.swap sell "exact" from 2 pools, and buys only within 1 ppm |

## 3. pump.fun BUY investigation

**Reserve semantics — `OBSERVED`, POST-trade.** Over 60 consecutive same-curve
event pairs, `Δreserve == event.amounts` for the *second* event (60/60),
i.e. each `TradeEvent` stores the state **after** its trade. Therefore the
pre-trade reserves for trade *N* are event *N−1*'s reserves.

**Instruction partition — `OBSERVED`.** `TradeEvent.ix_name` (offset 258) takes
values `sell`, `buy`, `buy_exact_sol_in`, `buy_exact_quote_in`. Sample: 147
sells, 54 `buy`, 20 `buy_exact_sol_in`, 32 `buy_exact_quote_in` (+ dust).

**SELL — `OBSERVED`, near-exact.** `gross = floor(vsol·tok/(vtok+tok))` is exact
for 126/147 sells; the remainder are pairs where the immediate predecessor was
not the prior trade on that curve (sample compaction), not a formula failure.

**BUY candidates (pre = predecessor, N = 282 events):**

| Candidate | Sequence | Rounding | Samples | Exact | Max err | Failure pattern |
|---|---|---|---:|---:|---:|---|
| `gross_cp` | `floor(vtok·sol/(vsol+sol))` | floor | 106 | 39 | ~1 lamport | all `*_exact_*`; mixed `buy` |
| `net_protofee` | net = `sol−sol·95/10000` | floor | 106 | 35 | large | exact-in |
| `net_total_fee` | net = `sol−fee−creator_fee` | floor | 106 | 35 | large | exact-in |
| **`sol_minus_1`** | `floor(vtok·(sol−1)/(vsol+sol−1))` | floor | 106 | **50** | ~1 unit on mixed | only non-exact-in |
| **`token_target`** | `sol = ceil(vsol·tok/(vtok−tok))` | ceil | 54 (`buy`) | **50** | ~1 unit | ordering/dust |

Per instruction:

| ix_name | `sol_minus_1` exact | `token_target` exact | `gross_cp` exact |
|---|---:|---:|---:|
| `buy_exact_sol_in` | **20/20** | n/a | 0/20 |
| `buy_exact_quote_in` | **30/32** | n/a | 0/32 |
| `buy` | 0/54 | **50/54** | 39/54 |

**Interpretation (`INFERRED`).** The curve is a pure constant product driven by
exact integer amounts:
* token-target (`buy`): the program solves for the **minimal SOL** to deliver
  `tok` tokens → `sol = ceil(vsol·tok/(vtok−tok))`; the event also carries the
  separate protocol/creator fees.
* exact-in (`buy_exact_sol_in`, `buy_exact_quote_in`): `tokens =
  floor(vtok·(sol−1)/(vsol+sol−1))`. The `−1` is `OBSERVED` and unexplained by
  fee accounting (fees are 95/30 bps, not 1 lamport); it is a real one-unit
  input reduction, not a reconstruction artifact (`HYPOTHESIS` on mechanism).

## 4. pump.swap BUY investigation

**Reserve semantics — `UNRESOLVED`.** Consecutive-pair tests are inconclusive
(pools are sampled sparsely). Treating the event's own `pool_*_token_reserves`
as pre-trade, the SELL formula `floor(pquote·base_in/(pbase+base_in))` is exact
for **405/646** sells in busy pools — 63%, not 100%.

**Fee regimes — `OBSERVED`.** Mismatching pools carry `lp_fee_basis_points = 2`
and `protocol_fee_basis_points = 93`, while the official `PUMP_SWAP_README`
documents `20` and `5`. Fees therefore vary per pool/version.

**Residual shape — `OBSERVED`.** Mismatches are always `predicted < observed`
(0/247 the other way) and reach a factor of ~25, far larger than any fee —
consistent with the effective reserve being adjusted by a **virtual quote
reserve** (`HYPOTHESIS`; cf. the official `NEGATIVE_VIRTUAL_QUOTE_RESERVES.md`
and the `Pool.virtual_quote_reserves: i128` field, which is negative in the
sampled pool).

**Layout — `UNRESOLVED`.** The sampled `Pool` account is 260 bytes; the
published IDL field list implies ~271 bytes, so the on-chain layout (or its
version) does not match the current IDL exactly.

**BUY candidates (pre = predecessor):** `gross_in`, `q_with`,
`uq_amount` → **0/242 exact** each. `MORE INVESTIGATION REQUIRED`.

## 5. Evidence sources

* Official pump.fun IDLs — `pump-fun/pump-public-docs` (`idl/pump.json`,
  `idl/pump_amm.json`).
* Official docs — `PUMP_PROGRAM_README.md` (Uniswap V2 synthetic reserves;
  `Global.fee_basis_points = 100`), `PUMP_SWAP_README.md`
  (`lp_fee_basis_points = 20`, `protocol_fee_basis_points = 5`),
  `NEGATIVE_VIRTUAL_QUOTE_RESERVES.md`.
* Real mainnet data via `rpc.solami.dev`: pump.fun 282 events
  (`pump.fun` program accounts, `TradeEvent`s); pump.swap 905 events across 34
  pools (`BuyEvent`/`SellEvent`). Raw samples cached in `/tmp` during the run.
* Tooling: `research/buy_parity.py` (collector) and
  `research/analyze_parity.py` (differential parity).

Sample sizes are **moderate** (282 / 905 events, tens of consecutive pairs).
They are sufficient to *identify* the root causes but **not** to certify 100%
parity; §12 specifies the confirmation set.

## 6. Candidate formulas

See the tables in §3 (pump.fun) and §4 (pump.swap). Candidate families tested:
fee-before-CP, fee-after-CP, protocol-only vs protocol+creator, `amount−1`,
`reserve+input` vs `reserve+net`, ceil vs floor, token-target inversion,
own-reserves vs predecessor-reserves.

## 7. Differential parity results

| Venue / direction | Best candidate | Exact | Verdict |
|---|---|---:|---|
| pump.fun sell | `floor(vsol·tok/(vtok+tok))` | 126/147 | near-exact (`OBSERVED`) |
| pump.fun `buy_exact_sol_in` | `floor(vtok·(sol−1)/(vsol+sol−1))` | **20/20** | exact on sample |
| pump.fun `buy_exact_quote_in` | same `−1` rule | **30/32** | exact on sample |
| pump.fun `buy` | `ceil(vsol·tok/(vtok−tok))` | **50/54** | exact on sample |
| pump.swap sell | `floor(pquote·base_in/(pbase+base_in))` | 405/646 | **not exact in general** |
| pump.swap buy | (grid) | 0/242 | **unresolved** |

## 8. Fee-state investigation

* pump.fun `TradeEvent` carries `fee_basis_points`, `fee`, `creator_fee_basis_points`,
  `creator_fee` — `OBSERVED` (95/30 bps in samples), while `Global.fee_basis_points = 100`
  in the docs. Fees can therefore change and differ from documentation.
* pump.swap `Buy`/`SellEvent` carry `lp_fee_basis_points` and
  `protocol_fee_basis_points` — `OBSERVED` as 2/93 on some pools, 20/5 documented.
* **Conclusion (`OBSERVED`):** a caller-supplied `fee_bps` is *not* sufficient
  for an authoritative quote. Authoritative fees must come from observed state
  (the event that produced the current reserves, and/or the venue config
  account), cached per market and exposed on `MarketState`.
* Minimum state: last observed `lp_bps` + `protocol_bps` (+ creator bps for
  pump.fun) and the slot/signature that produced them.

## 9. Root cause

1. **pump.fun:** M2.1 modeled one BUY. On-chain there are three, with two
   directions (token-target vs exact-in) and a one-lamport input reduction in
   the exact-in path. `VERIFIED` by the `ix_name`-partitioned parity table.
2. **pump.swap:** M2.1 generalized from two samples. Real pools vary in fee
   regime and (likely) carry a virtual-quote-reserve adjustment; the CP is not
   computed on the raw `pool_*_token_reserves`. `OBSERVED` + `HYPOTHESIS`.
3. **Fees:** modeled as a caller constant instead of observed state. `OBSERVED`.

## 10. Proposed implementation (NOT implemented)

**pump.fun**
1. Keep `TradeEvent.ix_name` on the decoded swap (add to `DecodedSwap`).
2. Split `quote_pumpfun` into:
   * `quote_buy_exact_in(sol) = floor(vtok·(sol−1)/(vsol+sol−1))`
   * `quote_buy_token_target(tok) = (sol = ceil(vsol·tok/(vtok−tok)), fees)`
   * `quote_sell(tok) = (gross = floor(vsol·tok/(vtok+tok)), fees)`
3. Report `fee_amount` from the observed rate; do **not** subtract fees from
   the curve input on the buy path.

**pump.swap**
1. Determine pre/post reserve semantics with dense per-pool chains
   (required before any formula).
2. Model effective reserves (pool reserves ± `virtual_quote_reserves`) and the
   observed LP/protocol fee pair.
3. Re-derive against `BuyEvent`/`SellEvent`; require 100% exact on the
   confirmation set before claiming parity.

**Fees**: add `lp_bps`, `protocol_bps`, `creator_bps`, `fee_slot`,
`fee_signature` to `MarketState`, populated from observed events; `quote` uses
them and rejects quotes when fee state is unknown/stale.

**Versioning**: select the formula from `venue + ix_name`; keep unknown
`ix_name` → `QuoteError::UnsupportedInstruction`.

**Safety**: unknown/stale reserves or fees, zero output, insufficient
liquidity and overflow all return deterministic `QuoteError`s (already the
shape); never panic.

**Performance**: unchanged shape — O(1) integer ops per quote, no allocation,
per-market state, no locks/RPC. Current quote is 66 ns.

## 11. Required data-model changes

| Field | Where | Why |
|---|---|---|
| `ix_name` (or enum) | `DecodedSwap` → `MarketState.last_ix` | select buy math |
| `lp_bps`, `protocol_bps`, `creator_bps` | `MarketState` | authoritative fees |
| `fee_slot`, `fee_signature` | `MarketState` | fee freshness/provenance |
| `virtual_quote_reserves` (already decoded for Pool) | `MarketState` | pumped.swap effective reserves |
| `direction`/`Side` on the quote request (exists) | `quote` API | explicit |

## 12. Test / acceptance plan

1. Collect **≥1000 labelled BUY events per instruction** (pump.fun) and
   **dense per-pool chains (≥200 pools × ≥10 consecutive events)** for
   pump.swap.
2. Differential harness (`research/analyze_parity.py`) must report **100% exact
   integer parity** per `(venue, ix_name, side)` before a formula is accepted.
3. Deterministic fixtures from the confirmed formulas + edge/overflow/fee/
   rounding tests.
4. SELL regression suite for both venues (including the virtual-reserve pools).
5. Keep the existing M1/M2/M2.1 suites green.

## 13. Risks and unresolved questions

* `OBSERVED` samples are moderate; the `−1` rule and token-target rule need
  ≥1000-sample confirmation before being called exact.
* pump.swap pre/post reserve semantics are `UNRESOLVED`; the `Pool` layout does
  not match the current IDL (260 vs ~271 bytes) — a layout/version problem.
* Virtual-quote-reserve adjustment is a `HYPOTHESIS` (official doc exists; the
  field is present and negative in a sampled pool) — not yet proven to close
  the residual.
* Non-SOL quote mints (`buy_exact_quote_in`) need dedicated samples; only 32
  were collected.
* Bonding-curve `complete`/real-reserve cap boundary was not the cause in
  samples but was not exhaustively exercised.
* M2.1's `tests/quote_parity.rs` overstates pump.swap sell parity; it should be
  retitled/scoped as part of the fix.

## 14. Recommendation

**`MORE INVESTIGATION REQUIRED`** before a production fix.

* **pump.fun — ready to fix** (`OBSERVED` + `INFERRED`): implement the
  instruction-aware formulas, but confirm on a ≥1000-sample labelled set first.
* **pump.swap — must not be fixed yet**: pre/post semantics, effective
  reserves and fee regime are unresolved; a fix now would repeat M2.1's
  two-sample mistake.

Fix scope if authorized: `src/decode/*` (carry `ix_name`, fees,
virtual reserves), `src/market.rs` (fee/ix state), `src/quote.rs`
(instruction-aware formulas), `tests/quote_parity.rs` (correct the overstated
claims) + new large-sample parity tests.
