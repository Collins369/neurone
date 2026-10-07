# Neurone — M2.2B Final Protocol Parity Report

**Investigation only.** No quote/state/execution logic was implemented. The only
non-research change is credential-plumbing compatibility (see §13) required for
the renamed credentials in `.env`.

Labels: `VERIFIED`, `OBSERVED`, `INFERRED`, `HYPOTHESIS`, `UNRESOLVED`.

---

## 1. Executive conclusion

* **pump.fun** — SELL and both exact-in BUYs are **exact** on the labelled
  sample. Because pump.fun events store **post-trade** reserves, each event's
  pre-trade state is derived directly as `post − delta`, which removes the
  reconstruction fragility that limited M2.2A:
  * SELL `floor(vq·base_in/(vb+base_in))` — account-grounded **45/45**.
  * `buy_exact_sol_in` `floor(vb·(sol−1)/(vq+sol−1))` — **56/56**.
  * `buy_exact_quote_in` — **36/36**.
  * token-target `buy` — direction identified, **not** certified (44/96);
    **excluded**.
* **pump.swap** — reserves are **pre-trade** (`VERIFIED` vs token balances) and
  the effective quote reserve is `raw_quote + signed virtual_quote_reserves`
  (`VERIFIED`). SELL is **exact** in the `(2,93)` and `(25,5)` regimes
  (**111/111**, **131/131**). BUY is exact only in `(25,5)` (51/53) — **not**
  exact, so excluded. The `(20,5)` regime needs a trailing field
  (`@279 = 920819`) plus pool state **at trade time**, which the event does not
  carry — **excluded**.
* **Verdict: `PROTOCOL PARITY CLOSED WITH EXCLUSIONS`.**

## 2. pump.fun BUY certification

Method: `TradeEvent` reserves are post-trade (78/78 consecutive pairs), so for
each event `pre = post − delta` (buy: `pre_vq = vq−sol`, `pre_vb = vb+tok`).
This is exact per event and needs no neighbour. Sample: 462 events
(271 sells, 96 `buy`, 56 `buy_exact_sol_in`, 36 `buy_exact_quote_in`).

| Instruction | Formula | Exact |
|---|---|---:|
| `sell` | `floor(pre_vq·tok/(pre_vb+tok))` | 219/271 (account-grounded 45/45) |
| `buy_exact_sol_in` | `floor(pre_vb·(sol−1)/(pre_vq+sol−1))` | **56/56** |
| `buy_exact_quote_in` | same | **36/36** |
| `buy` (token-target) | `sol = ceil(pre_vq·tok/(pre_vb−tok))` | 44/96 |

The `−1` input reduction is `OBSERVED` (100% of exact-in samples) and is **not**
fee-related (fees are 95/30 bps ≫ 1 lamport). Its mechanism is a
`HYPOTHESIS`; only the behaviour is verified. Boundary cases observed: 1-lamport
and dust buys still use `−1`; `real_token_reserves` never bounded an output; no
buy hit the completion cap. Zero/one-output and overflow boundaries are handled
by `u128` checked arithmetic in the (unimplemented) proposal.

The 52 sells that miss the self-derived formula are sells whose reserve delta
`≠ (sol, tok)` — i.e. their stored reserves are not the immediate successor of
the trade — a data/ordering artifact, not a formula error (the account-grounded
subset is 45/45).

## 3. PumpSwap `(20,5)` SELL

Effective quote = `raw_quote + virtual_quote_reserves` leaves a residual of
`+~920,619`. A trailing field in the pool account resolves it:

| offset | value | meaning |
|---|---|---|
| 245 | `i128` `17,583,583,979` | `virtual_quote_reserves` |
| **279** | `u64/u32` **`920,819`** | additional pool adjustment term |

With `q_eff = raw_quote + virtual_quote_reserves + 920,819`, 4/8 sells are exact
and 4/8 differ by exactly `−1`. The required value lies in a range containing
`920,819` for 4/8 and just outside it for 4/8, consistent with the field being
read from the **current** account while the trades used the pool's state **at
trade time**. The event carries only raw reserves, so the historical virtual /
adjustment state is not reconstructible from the event stream.
**`UNRESOLVED` — precise blocker: trade-time virtual/base adjustment is not
carried in the event.**

## 4. PumpSwap BUY

Single-event transactions, effective quote reserve:

| input | exact | regime |
|---|---:|---|
| `user_quote_amount_in` | 51/79 | 51/53 `(25,5)`; 0 elsewhere |
| `user_quote_amount_in − 1` | 51/79 | 15/21 `(2,93)` |
| `quote_amount_in` | 8/79 | — |
| `quote_amount_in − lp_fee − protocol_fee` | 50/79 | 50/53 `(25,5)` |

Not exact in any regime → **excluded**. (pump.fun BUY math does not transfer.)

## 5. Fee / version analysis

* Observed `(lp_bps, protocol_bps)`: `(25,5)` ×184, `(2,93)` ×132, `(20,5)`
  ×13 — the documented `20/5` is **not** universal; fees are **per-pool**
  (`OBSERVED`).
* Pool account lengths: `211, 243, 244, 245, 261, 271, 287, 300, 301`
  (`VERIFIED`). The published IDL layout is **271 bytes**; `300/301` append
  ~30 bytes (the trailing `@279` field lives in that appended region). Version
  is **by account length**.
* Authority for a quote is the **event's own** `lp_fee_basis_points` /
  `protocol_fee_basis_points` (and pump.fun's `fee_basis_points` /
  `creator_fee_basis_points`) — never a caller constant, never assumed from
  current docs.

## 6. Yellowstone observability

| Required field | Yellowstone source | Update event | Hot-path | RPC |
|---|---|---|---|---|
| market identity (mint/curve/pool) | account / event | Create/Swap | yes | no |
| raw reserves | `TradeEvent` (pump.fun, post) / `Buy·SellEvent` (pump.swap, pre) | swap | yes | no |
| virtual quote reserve | Pool **account** | account update | yes | no |
| trailing adjustment (vs. trade time) | Pool **account** at trade time | account update | **no** — not available at trade time | no |
| fee bps (lp/protocol/creator) | swap event | swap | yes | no |
| reserve freshness | slot watermark | slot | yes | no |

Everything the **supported** formulas need is Yellowstone-observable without
RPC. The unsupported regimes need pool state *at trade time* that the event does
not carry (it would require subscribing to and tracking pool accounts before the
trade, and even then a value change between update and trade is not detectable).

## 7. Exact formula table (supported)

| Venue | Instruction | Formula (raw integer) |
|---|---|---|
| pump.fun | sell | `gross = floor(vq·base_in/(vb+base_in))`; `net = gross − floor(gross·(10000−bps)/10000)` |
| pump.fun | `buy_exact_sol_in` / `buy_exact_quote_in` | `tokens = floor(vb·(sol−1)/(vq+sol−1))` |
| pump.swap | sell `(2,93)`,`(25,5)` | `gross = floor((raw_q+virt_q)·base_in/(raw_b+base_in))`; fees from event bps |

## 8. Exact parity statistics

| Venue / instruction | Regime | Sample | Exact | Max rel err |
|---|---|---:|---:|---:|
| pump.fun sell (account-grounded) | — | 45 | **100%** | 0 |
| pump.fun sell (self-derived) | — | 271 | 81% | ordering artifact |
| pump.fun `buy_exact_sol_in` | — | 56 | **100%** | 0 |
| pump.fun `buy_exact_quote_in` | — | 36 | **100%** | 0 |
| pump.fun `buy` (token-target) | — | 96 | 46% | not exact |
| pump.swap sell | `(2,93)` | 111 | **100%** | 0 |
| pump.swap sell | `(25,5)` | 131 | **100%** | 0 |
| pump.swap sell | `(20,5)` | 8 | 50% (±1) | 1e-11 |
| pump.swap buy | `(25,5)` | 53 | 96% | not exact |
| pump.swap buy | `(2,93)` | 21 | 71% | not exact |

## 9. Remaining unresolved items

* pump.swap `(20,5)`: ±1 after the trailing-field term; trade-time pool state
  unavailable (`UNRESOLVED`).
* pump.swap BUY: input source differs by regime; no regime exact.
* pump.fun token-target `buy`: needs the inverse/simulation path; not certified.
* pump.fun `−1`: behaviour `OBSERVED`, mechanism `HYPOTHESIS`.
* 287/300/301-byte tail beyond `@279` (all zeros in samples).

## 10. Supported vs unsupported regimes

**Supported (exact, Yellowstone-only):**
* pump.fun SELL (any fee regime).
* pump.fun `buy_exact_sol_in`, `buy_exact_quote_in`.
* pump.swap SELL with `(lp,protocol)` ∈ {`(2,93)`, `(25,5)`} and
  `virtual_quote_reserves = 0` or a stable observed value.

**Unsupported — deterministic exclusions:**
* pump.fun instruction `buy` (token-target) — needs an inverse solve.
* pump.swap venue, regime `(20,5)` — missing trade-time pool state.
* pump.swap BUY, all regimes — no exact formula.
* any pump.swap pool whose virtual/adjustment state is not observable before the
  trade.
* any market with unknown or stale reserves/fees.

## 11. Final production recommendation

Implement, when authorised: instruction-aware pump.fun quotes (sell + exact-in
buy) and pump.swap sell for the two proven regimes, gated on observed fee state,
effective reserves and freshness. Everything in §10's unsupported list must
return a deterministic `QuoteError::UnsupportedRegime`/`MissingState` rather
than a guess. Files to change: `src/decode/*` (carry `ix_name`, fee bps, signed
virtual reserves), `src/market.rs` (fee + ix + effective-reserve state),
`src/quote.rs` (regime dispatch + exclusions), `tests/quote_parity.rs` (rescind
the overstated pump.swap claim).

## 12. Representative signatures/slots

Pump.fun and pump.swap samples were collected with signatures/slots cached in
the research harness (`/tmp/pf_samples.json`, `/tmp/ps_single.json`); e.g.
pump.swap `(20,5)` pool `14LR8AFP69bFScEeEc5CunkLdiHTnpf5QELeDQ4sqX5` (len 301,
`virtual_quote_reserves = 17,583,583,979`, trailing field `920,819`), and
pump.fun curve `112heubZsHqSNJpc3QHTz6FjQDTLQpXiSDGyjALEZGr` with its
`TradeEvent` fixture in `fixtures/`.

## 13. Deviation: credential aliases

The operator renamed the environment credentials to `SOLAMI_RPC_API_KEY` /
`SOLAMI_GRPC_API_KEY` (and `SOLAMI_GRPC_YELLOWSTONE_ENDPOINT`). The live path
failed closed until the new names were accepted, so the following minimal,
credential-only compatibility changes were made (verified live):
`src/config.rs`, `src/ingest/solami.rs`, `tests/live_solami.rs`,
`research/*.py` accept the new names as aliases. No quote/state logic changed.

---

**PROTOCOL PARITY CLOSED WITH EXCLUSIONS**
