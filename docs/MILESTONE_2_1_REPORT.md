# Neurone Milestone 2.1 — Correctness Hardening Report

Scope: harden the two M2 limitations (pump.swap reserve freshness; exact
executable pricing) without expanding into a trading system.
**No filters, qualification, arming, capital, Beam or trading logic.**

`NEURONE_BLUEPRINT.md` was not modified. M2's architecture is unchanged.

---

## 1. Original issues

From the M2 report (§10):

* **Issue A — pump.swap reserve freshness.** The `Pool` account does not store
  current base/quote reserves, so current reserve state came from observed
  `BuyEvent`/`SellEvent`s. There was no explicit notion of "reserves unknown",
  no provenance, and no protection against an older event overwriting newer
  reserve state.
* **Issue B — exact executable pricing.** M2 exposed a constant-product
  `executable_price` primitive that "may differ from exact on-chain integer
  rounding by a raw unit".

## 2. Root cause

* **A:** `MarketState::apply_swap` overwrote reserves on every swap with no
  slot ordering guard and no `reserves_known` flag, so a late/out-of-order
  event could regress reserves, and consumers could not tell "no reserves yet"
  from "reserves are zero". The pool account cannot repair this (it stores no
  reserves), so the state itself must carry provenance + freshness.
* **B:** `executable_price` was a single ad-hoc expression per venue,
  un-parameterised by fees, with no rounding direction stated, no overflow
  protection, and no per-venue formula separation.

## 3. Implementation

| File | Change |
|---|---|
| `src/market.rs` | `ReserveState` (Unknown/Known/Stale); `reserves_known`, `last_reserve_slot`, `last_reserve_timestamp`, `last_reserve_signature`; `reserve_state()` / `is_reserve_state_fresh()`; slot-ordered reserve guard in `apply_swap`; `apply_swap` now takes the event slot + signature |
| `src/quote.rs` (new) | Exact integer quote engine: `Side`, `QuoteError`, `Quote`, `quote(..)`, per-venue `quote_pumpfun` / `quote_pumpswap`; checked arithmetic; explicit rounding |
| `src/shard.rs` | passes event slot + signature into `apply_swap` |
| `src/config.rs`, `config/default.toml` | `[market] reserve_stale_slots` (infrastructure freshness bound) |
| `src/bench.rs`, `src/main.rs` | quote latency microbenchmark |
| `tests/reserve_state.rs`, `tests/quote_parity.rs` (new) | reserve-state and parity tests |
| `tests/live_solami.rs` | live reserve-state + quote assertion |

## 4. Reserve-state model (Fix A) — **FIXED**

`MarketState` now carries, for every venue:

```
reserves_known: bool
last_reserve_slot: u64
last_reserve_timestamp: Option<i64>
last_reserve_signature: Option<[u8; 64]>
base_reserve / quote_reserve   (raw units, unchanged)
```

Sources of authoritative reserves:

* **pump.fun** — the `BondingCurve` account (real + virtual reserves) and, as a
  fallback, `TradeEvent`s.
* **pump.swap** — `BuyEvent` / `SellEvent`, which carry the latest observed
  pool reserves. A valid event supplies the latest observed reserves and
  updates the state directly; the whole historical trade sequence is **not**
  replayed.

**Ordering guarantee.** `apply_swap` updates reserves only when
`!reserves_known || event_slot >= last_reserve_slot`. An older (out-of-order)
event cannot overwrite newer reserve state; a replayed transaction is already
suppressed by the per-shard signature ring, so it updates nothing.

**Duplicate/replay idempotency** is preserved (verified by test).
**Pool independence** is preserved (each market owns its own reserves; verified
by test with two pools in different shards).

## 5. Freshness model (Fix A) — **FIXED**

```
UNKNOWN  -> no authoritative reserves observed yet
KNOWN    -> observed within `reserve_stale_slots` of the current slot
STALE    -> observed, but the current slot is further ahead than the bound
```

API: `MarketState::reserve_state(current_slot, stale_slots) -> ReserveState`
and `is_reserve_state_fresh(current_slot, stale_slots) -> bool`.

`reserve_stale_slots` is configuration (`[market].reserve_stale_slots`,
default 150 ≈ 60 s at 400 ms slots). It is an **infrastructure freshness
bound**, not a strategy rule; M3 will use it to decide whether a market has
"current executable state".

Freshness is computed from the stream itself (slot watermarks), never from an
RPC poll. **No per-market RPC polling was introduced**; the only RPC use remains
offline validation and fixture capture.

## 6. Exact quote engine (Fix B) — **FIXED** (sell) / **PARTIALLY FIXED** (buy)

```text
OBSERVED MARKET STATE -> EXACT QUOTE ENGINE -> (future) qualification/arming
```

`quote(market, side, input_amount, fee_bps, current_slot, stale_slots)` returns
`Result<Quote, QuoteError>` with:
`venue, side, input_amount, gross_output, fee_amount, net_output,
effective_price, valid`. No strategy fields.

Deterministic errors: `ReservesUnknown`, `ReservesStale`, `UnsupportedVenue`,
`ZeroInput`, `ZeroReserves`, `InsufficientLiquidity`, `InvalidFee`, `Overflow`.
Malformed state never panics.

### pump.swap (constant product on pool reserves)

* **buy:** `net_in = input - input*fee_bps/10000`; `base_out = floor(base *
  net_in / (quote + net_in))`. Fees come from the quote input.
* **sell:** `quote_gross = floor(quote * base_in / (base + base_in))`;
  `quote_net = quote_gross - quote_gross*fee_bps/10000`. Fees come from the
  quote output.

### pump.fun (Uniswap-V2-style virtual reserves)

* **sell:** `quote_gross = floor(v_quote * base_in / (v_base + base_in))`;
  `quote_net = quote_gross - quote_gross*fee_bps/10000`.
* **buy:** same constant-product form on the net input.

### Parity status

| Direction | Status | Evidence |
|---|---|---|
| pump.swap **sell** | **FIXED — exact parity** | 2 real `SellEvent`s reproduce `quote_amount_out` exactly |
| pump.fun **sell** | **FIXED — exact parity** | real `TradeEvent` reproduces `sol_amount` (987,653) and `fee` (9,383) exactly |
| pump.swap **buy** | **PARTIALLY FIXED** | real `BuyEvent` reproduced to < 1 ppm; exact integer parity not established |
| pump.fun **buy** | **PARTIALLY FIXED** | documented Uniswap-V2 form; exact on-chain buy parity not established |

Sell-side parity is exact. Buy-side residual is ~1 lamport's worth of base
tokens: on real data the observed buys sit a hair below the pure constant
product, and the residual scales with `reserve_base/reserve_quote` (i.e. with
one quote-unit worth of tokens). Neither the documented global fee (100 bps) nor
the event fee fields (95 bps + creator 30 bps) explain it, and the programs are
closed-source, so no exact buy formula is claimed.

## 7. Protocol sources

* Official pump.fun IDLs — `pump-fun/pump-public-docs` → `idl/pump.json`,
  `idl/pump_amm.json` (layout/discriminators).
* Official pump.fun program docs — `docs/PUMP_PROGRAM_README.md` ("The bonding
  curve formula is based on Uniswap V2 and uses synthetic x and y reserves";
  `Global.fee_basis_points = 100`) and `docs/PUMP_SWAP_README.md`
  (`lp_fee_basis_points = 20`, `protocol_fee_basis_points = 5`).
* Real mainnet events via `rpc.solami.dev` for empirical fitting/verification.

## 8. Integer / rounding behaviour

* All arithmetic is `u128`/`i128` with `checked_mul`/`checked_add`; overflow
  returns `QuoteError::Overflow` (never panics).
* Fees: `net = amount*(10000 - bps)/10000` with **floor**; `fee = amount - net`.
  This is the direction the real sell events exhibit.
* Constant product: `floor(reserve_out * net_in / (reserve_in + net_in))`.
* Zero result from a non-zero input is reported as `InsufficientLiquidity`, not
  silently returned as a zero quote.
* No floating point anywhere on the quote path.

## 9. Tests

`cargo test` → **81 deterministic tests pass**, 1 live test `#[ignore]`d by
default (passes when run with a credential).

New/updated:

* `src/quote.rs` unit tests (8): exact buy/sell, fee direction, stale/unknown,
  zero input, zero reserves, invalid fee, overflow, insufficient liquidity.
* `tests/quote_parity.rs` (5): exact pump.swap sell ×2, exact pump.fun sell,
  bounded pump.swap buy residual (documented limitation), unknown-reserves.
* `tests/reserve_state.rs` (6): unknown represented, stale detected, first swap
  establishes reserves, newer replaces, older cannot overwrite, replay
  idempotent, pools independent.
* All M1/M2 tests remain green (architecture, decode/state, reconnect, real
  fixtures, market/volume).

## 10. Live validation

Real, authenticated Solami Yellowstone (release binary, 22 s):

* ~1,640 events/s; 48,055 swaps decoded (pump.fun 7,157 / pump.swap 40,898);
  3,314 markets; `decode_rejected=0`, `stale_events=0`, `reconnects=0`.
* decode p50 5 µs / p99 50 µs; state update p50 5 µs / p99 10 µs;
  end-to-end p50 100 µs / p99 5 ms.
* Live test additionally asserts ≥1 market has `reserves_known` and computes a
  real quote on a live pump.swap market, e.g.
  `PumpSwap sell 100000 -> gross 85,502,365 fee 213,756 net 85,288,609`.

No credentials are logged or persisted; normal tests remain network-independent.

## 11. Performance

Release build, 1 vCPU, 8 shards.

| Measurement | Result |
|---|---|
| `quote` (buy/sell) | **66 ns/op**, p50 58 ns, p95 90 ns, p99 96 ns |
| Protocol decode | 769,749 decodes/s, 1,299 ns/decode |
| Engine burst | 260,072 events/s (200k events) |
| Engine steady 10k ev/s | e2e p50 50 µs / p99 250 µs; proc p50 1 µs |
| Live decode / update / e2e (p50) | 5 µs / 5 µs / 100 µs |

No regression versus M2 (M2: 259,056 ev/s, 766,213 decodes/s). The quote engine
is allocation-free and lock-free, so it adds no serial bottleneck; reserve
updates stay per-shard.

## 12. Limitations / assumptions

1. **pump.fun buy parity — REMAINS A LIMITATION.** Exact on-chain buy parity
   could not be established from public sources; the model matches the
   reserve-move structure and is within ~1 lamport's worth. Marked PARTIALLY
   FIXED and pinned by a bounded-residual test.
2. **pump.swap buy parity — REMAINS A LIMITATION.** Reproduced to < 1 ppm, not
   exact.
3. Fee bps is a caller parameter (venue/side); callers should use the venue's
   observed fee. The engine does not read `Global`/`GlobalConfig` accounts yet.
4. pump.swap reserves are only known after the first observed swap (the `Pool`
   account has none); a freshly created pool is `Unknown` until then — this is
   the intended model.
5. Freshness uses slot distance, not wall-clock time; slot distance is the
   chain-native measure and needs no RPC.

## 13. Recommended M3

Build deterministic qualification on these primitives: rolling 5-minute volume
and acceleration from the per-slot window, liquidity/market-cap filters, and a
deterministic safety/executability state machine that consumes
`reserve_state()`/`quote()`. Also decode mint decimals (UI-unit prices) and
resolve the pump.fun buy residual (e.g. by disassembling the program or
collecting a larger labelled sample). Do not start M3 without authorization.
