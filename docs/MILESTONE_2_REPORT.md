# Neurone Milestone 2 — Completion Report

Scope: **Milestone 2 only** — real protocol-derived market state (pump.fun
bonding curve + pump.swap AMM), preserving M1's parallel/sharded architecture.
**No trading, strategy, arming, capital or Beam work.**

Readiness: **implemented, unit tested, integration tested, and live verified**
against authenticated Solami Yellowstone. Both venues were decoded from real
mainnet events; no unverified live claim remains for the decoded surfaces.

---

## 1. Implementation summary

M2 adds a deterministic protocol-decoding layer between normalization and the
sharded state engine, and extends `MarketState` with protocol-derived
primitives:

```text
Yellowstone SubscribeUpdate
   │  (ingestion boundary)
   ▼
normalize + decode  ──►  AccountUpdate{ decoded }        (route by pubkey)
                    └──►  TransactionUpdate{ swaps, creates }  (route by market key)
   ▼
hash(market) → shard → MarketState{ reserves, price, volume, lifecycle }
   ▼
lock-free telemetry (decode latency, decoded/rejected, swaps per venue, volume)
```

Two venues, both decoded from **official pump.fun IDLs**
(`pump-fun/pump-public-docs`):

* **pump.fun bonding curve** (`pump`) — `BondingCurve` account + `TradeEvent`.
* **pump.swap AMM** (`pump_amm`) — `Pool` account + `BuyEvent`/`SellEvent`.

## 2. Changed M1 files

| File | Change |
|---|---|
| `src/decode/` (new) | `borsh.rs`, `pda.rs`, `pumpfun.rs`, `pump_amm.rs`, `mod.rs` |
| `src/lib.rs` | adds `decode` module |
| `src/events.rs` | `AccountUpdate.decoded`, `TransactionUpdate.{swaps,creates,decode_rejected}`, program-log decoding, market keys from decoded swaps |
| `src/market.rs` | protocol-derived fields, `Ratio`, `VolumeWindow`, `executable_price`, slot-aware volume; rewritten |
| `src/shard.rs` | applies decoded accounts/swaps/creates; venue/volume/stale/decode telemetry |
| `src/telemetry.rs` | decode histogram + decoded/rejected/volume/stale/swaps-per-venue counters |
| `src/config.rs` | default transaction programs = pump.fun + pump.swap; endpoint alias `SOLAMI_YELLOWSTONE_ENDPOINT`; account filters opt-in |
| `src/ingest/solami.rs` | records normalize+decode latency |
| `src/ingest/simulated.rs` | synthetic swaps so the bench exercises the swap path |
| `src/bench.rs`, `src/main.rs` | decode microbenchmark; per-venue + decode output |
| `config/default.toml`, `README.md`, `.env.example` | M2 configuration/docs |
| `fixtures/` (new) | real mainnet decoder fixtures |
| `tests/real_fixtures.rs`, `tests/decode_and_state.rs`, `tests/live_solami.rs` (new) | real-data, state, and live tests |

`NEURONE_BLUEPRINT.md` was **not** modified.

## 3. Solami documentation consulted

Per task §3 the official index was re-read first: `https://solami.dev/llms.txt`
→ `/docs/grpc`, `/docs/endpoints`, `/docs/policies-limits`, `/docs/errors`
(research record: `docs/SOLAMI_RESEARCH.md`). The M1 interface was preserved
(`https://grpc.solami.dev:443`, `x-token`, `geyser.Geyser/Subscribe`,
`yellowstone-grpc-client`/`-proto`, `processed`, `from_slot`, scoped filters);
nothing in the current docs contradicted it.

Protocol layouts came from the **official pump.fun IDLs**, not from memory:
`idl/pump.json`, `idl/pump_amm.json` (`pump-fun/pump-public-docs`).

## 4. Decoded protocols and exact layouts

All Anchor accounts/events are **8-byte discriminator + Borsh fields**.
Discriminators are taken verbatim from the IDLs. Borsh integers are
little-endian; `bool` is 1 byte; pubkeys are 32 bytes.

### pump.fun `BondingCurve` (disc `[23,183,248,55,96,216,172,96]`)

| off | field | type |
|---|---|---|
| 0 | discriminator | [u8;8] |
| 8 | `virtual_token_reserves` | u64 |
| 16 | `virtual_quote_reserves` | u64 |
| 24 | `real_token_reserves` | u64 |
| 32 | `real_quote_reserves` | u64 |
| 40 | `token_total_supply` | u64 |
| 48 | `complete` | bool |
| 49 | `creator` (optional) | pubkey |
| 81 | `is_mayhem_mode`, `is_cashback_coin` (optional) | bool ×2 |
| 83 | `quote_mint` (optional) | pubkey |
| 115 | `creator_fee_bps` (optional) | u64 |

Total 125 bytes for current accounts; the legacy 49-byte layout decodes too
(trailing fields become `None`). Verified against a real 125-byte account.

### pump.fun `TradeEvent` (disc `[189,219,127,211,78,230,97,238]`)

`mint`(32), `sol_amount`(u64), `token_amount`(u64), `is_buy`(bool),
`user`(32), `timestamp`(i64), `virtual_sol_reserves`(u64),
`virtual_token_reserves`(u64), `real_sol_reserves`(u64),
`real_token_reserves`(u64), then `fee_recipient`(32), `fee_basis_points`(u64),
`fee`(u64), `creator`(32), `creator_fee_basis_points`(u64), `creator_fee`(u64).
The required prefix is read always; the fee tail is read when present. Later
fields (shareholders vec, quote mint, mayhem/cashback config) are ignored.

`CreateEvent` (disc `[27,114,169,77,222,235,99,118]`) is decoded to seed a
market: name/symbol/uri strings, `mint`, `bonding_curve`, …, timestamp, virtual
and real reserves, `quote_mint`.

**Bonding-curve PDA:** `find_program_address(["bonding-curve", mint], pump)`.

### pump.swap `Pool` (disc `[241,154,109,4,17,177,109,188]`)

`pool_bump`(u8), `index`(u16), `creator`(32), `base_mint`(32),
`quote_mint`(32), `lp_mint`(32), `pool_base_token_account`(32),
`pool_quote_token_account`(32), `lp_supply`(u64), `coin_creator`(32),
`is_mayhem_mode`(bool), `is_cashback_coin`(bool), `virtual_quote_reserves`(i128).
The pool does **not** store current reserves (they live in the pool token
accounts), so reserve/price primitives come from swap events.

### pump.swap `BuyEvent` / `SellEvent`
(disc `[103,244,82,31,44,245,119,119]` / `[62,47,55,10,165,3,220,42]`)

Shared leading sequence: `timestamp`(i64), `base_amount`(u64),
`limit`(u64), `user_base_token_reserves`(u64), `user_quote_token_reserves`(u64),
`pool_base_token_reserves`(u64), `pool_quote_token_reserves`(u64),
`quote_amount`(u64), `lp_fee_bps`(u64), `lp_fee`(u64), `protocol_fee_bps`(u64),
`protocol_fee`(u64), `quote_*_with_fee`(u64), `user_quote_amount`(u64),
`pool`(32), `user`(32), user base/quote token accounts, fee recipients,
`coin_creator`(32), `coin_creator_fee_bps`(u64), `coin_creator_fee`(u64).
Only this shared prefix is read; later fields diverge and are ignored.

## 5. Price / liquidity / volume formulas

All values are **raw integer units** (`base_reserve` = raw base-token units;
`quote_reserve` = raw quote units, lamports when quote is wrapped SOL). Prices
are exact integer ratios, never floats, so state stays deterministic.

* **Reference / spot price** = `quote_reserve / base_reserve` (prefers virtual
  reserves for the curve). Returns `None` if either side is zero — a zero side
  is "no price", not "price 0".
* **Executable price** (constant product with fees), for input size `amount`
  and fee `bps`:
  * buy: `net = amount·(10000−bps)/10000`;
    `tokens_out = x·net/(y+net)`; price = `amount / tokens_out`.
  * sell: `tokens_out = y·amount/(x+amount)`;
    `net = tokens_out·(10000−bps)/10000`; price = `net / amount`.
  This is the *average realised* price including impact and fees, not the
  reserve ratio. It is a primitive only — no trades are constructed.
* **Liquidity** = the protocol's real base/quote reserves.
* **Volume primitive** = per-slot, per-market `buy_quote`, `sell_quote`,
  `buy_base`, `sell_base`, `trades`.

`Ratio::scaled(base_decimals, quote_decimals)` exists to move to UI units
explicitly; decimals (which live on mint accounts) are not read in M2, so no
implicit scaling happens anywhere.

## 6. MarketState contract

`MarketState` (M1 fields retained) now carries, where derivable per venue:
`venue`, `base_mint`, `quote_mint`, `creator`, pool token accounts, `last_slot`,
`base_reserve`, `quote_reserve`, `virtual_base_reserve`,
`virtual_quote_reserve`, `token_total_supply`, `complete`, trade counts,
`last_trade_slot`, `last_trade_timestamp`, `last_fee_bps`, and a bounded
`VolumeWindow`. Reserve/volume fields are `u64`/`u128`/`i128` raw units.

`size_of::<MarketState>() = 592 bytes` inline (plus a sparse, capped
1024-bucket volume window that only grows for slots that actually traded).

## 7. Live-event validation

Real, authenticated Solami Yellowstone, release binary.

**Both venues, 30 s (transactions for both programs + slots + blocks-meta):**

* `swaps_pumpfun = 6,849`, `swaps_pumpswap = 55,778` (~2,087 swaps/s); markets ≈ 3,258.
* `decode_rejected = 0`, `stale_events = 0`, `reconnects = 0`.
* decode p50 50 µs / p99 1 ms; proc p50 10 µs.

(An earlier run exposed a real bug: pump.swap events (~450–500 B) exceeded the
program-data size cap and were silently skipped. The cap was raised to 2048
bytes and checked on decoded length; the pump.swap counts above are from after
the fix.)

**Both venues + 16 real accounts subscribed, 20 s:**

* `events_received = 22,020` (~1,427/s), `events_decoded = 56,796`.
* `swaps_pumpfun = 10,116`, `swaps_pumpswap = 46,562`; markets ≈ 3,393.
* decode p50 5 µs / p99 50 µs; proc p50 5 µs / p99 10 µs; e2e p50 100 µs / p99 5 ms.
* `decode_rejected = 0`, `stale_events = 0`, `reconnects = 0`.

**Independent mainnet cross-checks (via `rpc.solami.dev`):**

* PDA validated: `bonding_curve("DdtKUh…pump")` equals the real on-chain curve
  `112heubZsHqS…` (asserted in `tests/real_fixtures.rs`).
* 12/12 real `BondingCurve` accounts decoded with sane values
  (virtual ≈ 30 SOL / 1.073e15 base; real reserves consistent).
* 16/16 decoded pump.swap `pool` addresses were present in their transactions'
  account keys, confirming the event offsets.
* The real `BondingCurve` account and the real `TradeEvent` for the same market
  decode to **identical reserves**.

## 8. Tests and pass counts

`cargo test` → **62 deterministic tests pass** (0 failed); 1 live test is
`#[ignore]`d by default and passes when run with a credential.

* `src` unit tests (40): config, hash, normalization, market state, volume
  window, exact-ratio prices, Borsh reader, PDA, both decoders, telemetry,
  subscription shape, shard concurrency barrier, market-state size guard.
* `tests/real_fixtures.rs` (7): real `BondingCurve`, real `TradeEvent`, account
  ⇄ trade agreement, PDA vs mainnet, real `Pool`, real Buy/Sell events,
  malformed/wrong-program rejection.
* `tests/decode_and_state.rs` (6): venue separation, volume across slots,
  replayed-swap idempotence, deterministic swap sequences, create-event
  seeding, degenerate-reserve handling.
* `tests/architecture.rs` (8, M1): determinism, routing, ordering, duplicates,
  512-market concurrency, shutdown.
* `tests/ingest_reconnect.rs` (1, M1): reconnect after connect failures +
  stream interruptions.
* `tests/live_solami.rs` (1, ignored): authenticated stream decodes markets —
  verified passing (`swaps_pumpfun`/`swaps_pumpswap` both non-zero).

`cargo clippy --all-targets` → clean. `cargo fmt --check` → clean.

## 9. Performance

Release build, 1 vCPU, 8 shards.

| Measurement | Result |
|---|---|
| Synthetic burst throughput | **259,056 events/s** (200k events, 0.772 s), 318k state updates, shards balanced |
| Steady 10k ev/s | e2e p50 50 µs / p95 100 µs / p99 500 µs; proc p50 1 µs / p99 5 µs |
| Pure protocol decode (real fixtures) | **766,213 decodes/s**, **1,305 ns/decode** |
| Live decode (release, real load) | p50 5 µs / p99 50 µs |
| Live state update (release) | p50 5 µs / p99 10 µs |
| Live end-to-end (release) | p50 100 µs / p99 5 ms |
| Per-market inline memory | 592 bytes + sparse volume buckets (≤1024) |

Rolling-volume update cost is O(1) appended to a sparse per-slot bucket; the
0 rejected / 0 stale counts and single-digit-µs processing show decoding is not
a bottleneck and did not serialize the shards.

## 10. Limitations / assumptions

1. **Fee/precision:** executable price uses the constant-product formula with
   the observed fee bps; exact on-chain rounding may differ by a raw unit.
2. **pump.swap reserves** come from swap events (the `Pool` account does not
   store them); before the first observed swap a pool market has identity but
   no reserves.
3. **Quote mints:** pump.fun now supports non-SOL quote mints; volume uses
   `sol_amount`, correct for SOL-quoted markets. Non-SOL quote markets would
   need `quote_amount` handling in M3.
4. **Decimals:** base/quote mint decimals are not read, so prices are raw
   ratios; UI scaling is explicit via `Ratio::scaled`.
5. **Layout versioning:** decoders read a validated prefix of the *current*
   IDLs; a future layout change to the leading fields would need an IDL update.
6. **Market identity for pump.fun** is the bonding-curve PDA of the mint;
   trades are routed via that PDA, so no account subscription is required.
7. **Default config does not subscribe accounts by owner** (it would trigger a
   large startup snapshot); use `account_addresses` to track specific accounts.

## 11. Recommended M3

Deterministic qualification on top of these primitives: rolling volume windows
(5-minute) and acceleration, liquidity/market-cap filters, and a deterministic
safety/executability state machine — still no trading. M3 should also decode
mint decimals (to express prices in UI units) and add the second-venue
executable sell-route check that arming will need.

## 12. Stop condition

M2 is committed and work stops here. No filters/qualification/arming/capital/
Beam/trading logic was implemented. Milestone 3 must not begin without
operator authorization.
