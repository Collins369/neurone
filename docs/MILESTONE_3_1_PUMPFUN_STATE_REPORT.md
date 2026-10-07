# Neurone — M3.1: Pump.fun Yellowstone Trade-Time State

Adds a Pump.fun bonding-curve **account subscription** + state cache + slot
correlation on top of M3, and fixes the decoder variant that caused most of the
M3 "state-dependent" failures. **No trading/execution.**

Labels: `VERIFIED`, `OBSERVED`, `INFERRED`, `HYPOTHESIS`, `UNRESOLVED`.

---

## 1. Problem Confirmed

M3 reported pump.fun SELL / token-target `buy` as state-dependent, with a subset
of live events whose reconstructed reserves looked inconsistent (many with
`sol_amount = 0` and impossible reserves). The actual cause is now identified:

* **Dominant cause — non-SOL (USDC) quote pairs (`VERIFIED`, fixed).** The pump
  program now supports coins paired with a non-native quote mint. For those
  trades `TradeEvent.sol_amount == 0`; the value lives in the **appended**
  `quote_amount` / `virtual_quote_reserves` / `real_quote_reserves` fields (after
  `ix_name`, buyback/cashback fields and the `shareholders` vec). The M3 decoder
  always read the leading SOL fields, so USDC-pair events decoded to zeros —
  which M3 counted as `decoder_inconsistent` errors.
* **Residual — `UNRESOLVED`.** After the fix, ~2–20% of pump.fun SELL /
  token-target events still do not satisfy `x·y=k` under any tested
  interpretation (virtual, real, or fee-adjusted reserves), even though the
  decoded layout is internally consistent (`virtual − real = 279.9e12`, the
  fixed virtualisation constant). The account stream does not explain them
  either (see §7).

The quote **formula** is not the problem (verified again below).

## 2. Account Source

The required state is the `BondingCurve` account (PDA `["bonding-curve", mint]`),
which holds `virtual_token_reserves`, `virtual_quote_reserves`,
`real_token_reserves`, `real_quote_reserves`, `token_total_supply`, `complete`,
`creator`, `quote_mint`. The existing decoder
(`src/decode/pumpfun.rs::decode_account`) already decodes it (legacy 49-byte
through current 125-byte layouts).

Only the bonding curve is needed; the `Global`/`FeeConfig` accounts are not
required for the validated path (per-trade fee bps comes from the trade event).

## 3. Yellowstone Subscription

Reuses the existing `TonicConnector` — no second gRPC client. One narrow
`accounts` filter added to the existing subscribe request:

```text
accounts["pumpfun-bc"] = {
  owner = ["6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"],
  filters = [ memcmp{ offset = 0, base58 = "4y6pru6YvC7" } ]   // BondingCurve disc
}
```

The memcmp discriminator excludes the program's other account types (Global,
FeeConfig, volume accumulators), so only bonding curves stream. Config knobs
added: `ingest.filters.account_memcmp_base58`, `…account_data_slice_len`
(`accounts_data_slice` support). The transaction + slot + blocks-meta
subscriptions are unchanged.

Startup hydration: the filter streams existing curves as `is_startup = true`
updates. In the 60 s run the cache observed **3,073 curve updates** and held
**240 curves** (rates are chain/filter-limited, not processing-limited).
No RPC/REST polling is used.

## 4. State Cache

`src/validate.rs::CurveCache`, keyed by the bonding-curve pubkey (identical to
the market key derived from the mint), values = `CurveHistory`:

```text
states: ring of (slot, virtual_base: u128, virtual_quote: i128)   // newest-last
layout_len: usize                                                 // version by length
```

Exact integers only; no floating point. Freshness is by slot distance
(`KNOWN` within the configured bound, `STALE` beyond, `UNKNOWN` when absent),
mirroring `MarketState::reserve_state`. The ring keeps enough history to select
the state strictly older than a trade's slot, so a newer (post-trade) account
update cannot be mistaken for the pre-trade state; out-of-order updates are
inserted by slot, and same-slot updates are de-duplicated.

## 5. Correlation Algorithm

For a pump.fun trade at slot `T`:

1. `pre = post − delta` from the event itself (pump.fun events are post-trade).
2. Look up the curve cache; take the newest cached state with `slot < T`.
3. If it equals the event-derived pre-state (`vb` and `vq` both match) the
   pre-state is **corroborated** by the account stream; otherwise the account
   stream has not caught up (or an intermediate trade is missing) and only the
   event-derived pre-state is used.

**Multi-event transactions:** each event carries its own post-trade reserves, so
`pre = post − delta` is per event and does **not** depend on the previous event;
the M3 concern (multi-event transactions) is therefore handled without ordering
assumptions. Foreign events are excluded by program attribution (M3 fix,
preserved).

**Result of correlation (`OBSERVED`):** 2,762 correlations, **1,306 corroborated
(47%)**. The account stream lags the transaction stream (separate subscriptions,
arrival order differs), so slot-based pre-state selection corroborates about
half the trades and does not repair the residual — it is a validator/scanner
feed, not a pre-state oracle, in a single-pass harness.

## 6. Layout Handling

| Layout | Length | Decoder |
|---|---|---|
| legacy | 49 | supported (prefix only) |
| creator-fee | 81–83 | supported |
| quote-mint v2 | 115 | supported |
| current + fees | 125 | supported |
| extended | 141/150/151/256 | prefix supported; unknown trailing bytes ignored |
| anything else | — | `decode_account` returns `None` → `UNKNOWN_LAYOUT`, never guessed |

Account layout is length-gated; unknown lengths are rejected deterministically.
`TradeEvent` now decodes the appended tail (shareholders vec = `u32` count ×
`pubkey+u16`, then `quote_mint`, `quote_amount`, `virtual_quote_reserves`,
`real_quote_reserves`) and selects the quote side by `sol_amount == 0`.

## 7. Live Parity Results

Live Solami Yellowstone, release, 60 s: 68,560 updates, 20,385 swaps.

| Protocol | Instruction | Samples | Exact | Mismatch | Errors | Classification |
|---|---|---:|---:|---:|---:|---|
| PumpSwap | SELL | 9,012 | **9,012 (100%)** | 0 | 0 | Yellowstone-only exact |
| PumpSwap | `buy_exact_quote_in` | 5,555 | **5,555 (100%)** | 0 | 0 | Yellowstone-only exact |
| PumpSwap | `buy` | 2,767 | **2,767 (100%)** | 0 | 0 | Yellowstone-only exact |
| Pump.fun | `buy_exact_sol_in`/`_quote_in` | 647 | **647 (100%)** | 0 | 0 | Yellowstone-only exact |
| Pump.fun | SELL | 1,406 | 1,131 (80.4%) | 275 | 4 | state-dependent / unmodeled variant |
| Pump.fun | `buy` (token-target) | 994 | 709 (71.3%) | 285 | 0 | state-dependent / unmodeled variant |

Mismatch classification: `wrong trade-time state` / `missing Yellowstone
field/state` — the decoded reserves are layout-consistent but the event's
reserves do not correspond to the immediate pre/post of the trade for these
payloads. **No mismatch is a formula or rounding error** (the same formulas are
100% on PumpSwap and on pump.fun exact-in buys; the account-grounded check was
45/45 in M2.2A).

## 8. Regression Results

`cargo test` → **89 deterministic tests pass, 0 failed**, plus 1 `#[ignore]` live
test. M3 regression suite (`tests/m3_parity.rs`, 8 tests) unchanged and green:
pump.swap SELL exact, signed virtual reserves, negative effective reserve
rejected, pump.swap `buy_exact_quote_in` `−1`, pump.swap token-target buy,
pump.fun exact-in `−1`, pump.fun token-target `ceil`, strict instruction
classification. All M1/M2/M2.1/M3 tests remain green; clippy and fmt clean.

## 9. Performance

Release, 1 vCPU:

| Stage | p50 | p95 | p99 |
|---|---:|---:|---:|
| `normalize` + decode (per update) | 10 µs | 66 µs | 138 µs |
| quote + parity check (per swap) | 0.33 µs | 0.75 µs | 1.03 µs |

Throughput: 20,385 swaps / 60 s ≈ 340/s; 68,560 updates / 60 s ≈ 1,143/s;
3,073 curve-account updates / 60 s ≈ 51/s. No locks added to the quote path
(the cache is single-writer in the harness; the production shard owns its
markets, preserving the parallel design). Error/drop rate: 0.

## 10. Scanner Readiness

The bonding-curve account stream and the slot-versioned cache give the scanner a
deterministic, Yellowstone-only state feed keyed by market identity, with
`KNOWN`/`STALE`/`UNKNOWN` freshness — usable for continuous multi-market
tracking without RPC polling. The parallel sharded architecture is unchanged
(shards own their markets; no global lock). What is **not** yet ready: the
residual ~20–29% of pump.fun SELL/token-target events whose event reserves do
not correspond to the trade, which currently must be treated as **not
reconstructable** rather than quoted from.

## 11. Remaining Limitations

* **Residual pump.fun SELL / token-target events (`UNRESOLVED`).** ~20–29% of
  live events are layout-consistent but do not satisfy the constant product with
  the event's reserves; the account stream corroborates the pre-state for only
  ~47% of correlated trades (arrival ordering). Needs a follow-up focused on
  those specific payloads (e.g. mayhem/holder-reward/cashback variants,
  same-slot multi-trade ordering, or a further event variant).
* One pump.fun SELL with `base = 1` (dust) is rejected as `ZeroReserves`.
* Supported account layouts beyond the prefix (141/150/151/256) are decoded by
  prefix only; unknown trailing semantics are ignored (no quote impact).

## 12. Final Status

**`PASS WITH EXCLUSIONS`**

* **PASS:** PumpSwap SELL / `buy_exact_quote_in` / `buy` (100% live), pump.fun
  `buy_exact_sol_in` / `buy_exact_quote_in` (100% live). Bonding-curve account
  stream + slot-versioned cache + correlation are implemented without RPC
  polling and without weakening the parallel architecture.
* **EXCLUDED:** pump.fun SELL and token-target `buy` from the **event alone**
  still fail for a minority of live events; those markets must not be quoted
  from the event and require further decoder/correlation work before they can be
  treated as exact.

The target `PASS` was **not** reached: two pump.fun paths remain below 100%, so
the status is reported honestly as `PASS WITH EXCLUSIONS`. No execution work was
added.
