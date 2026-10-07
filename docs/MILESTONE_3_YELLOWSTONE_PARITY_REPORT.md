# Neurone — M3: Live Yellowstone Protocol-Parity Report

Validation milestone. **No trading/execution was implemented.** Reuses the M1/M2
Yellowstone foundation and the reconciled quote math; adds a live validation
harness and deterministic regression tests.

Labels: `VERIFIED`, `OBSERVED`, `INFERRED`, `HYPOTHESIS`, `UNRESOLVED`.

---

## 1. Existing Architecture Used

Nothing was duplicated; the live test drives the existing production path.

| Component | Reused |
|---|---|
| Yellowstone client + auth + reconnect | `src/ingest/solami.rs` (`TonicConnector`, `build_subscribe_request`, `Connector`) |
| Subscription filters | `config/default.toml` (pump.fun + pump_amm transactions, slots, blocks-meta) |
| Protobuf | `yellowstone-grpc-proto` 13.0.0 |
| Normalizer | `src/events.rs::normalize` (now with program attribution) |
| Decoders | `src/decode/{pumpfun,pump_amm}.rs` (extended, see §10) |
| Quote math | `src/quote.rs` (reconciled formulas added) |
| Telemetry/latency | `src/clock.rs` + a lock-free counter set in `src/validate.rs` |

Parallel sharded state (`src/shard.rs`/`src/engine.rs`) is untouched and still
covered by the M1 architecture tests; the validation harness intentionally does
not route into shards (it validates parity, not the trading path).

## 2. Yellowstone Connection

* Endpoint/credentials: from the project env (`SOLAMI_GRPC_API_KEY` /
  `SOLAMI_GRPC_ENDPOINT`); **never logged**. No credentials appear in this report.
* Service: `geyser.Geyser/Subscribe` over TLS, `x-token` metadata,
  commitment `processed`.
* Filters: `transactions` scoped to the pump.fun and pump_amm program ids,
  `slots` (interslot), `blocks_meta`.
* Behaviour: connected on first attempt, streamed continuously, no reconnect
  needed during runs; keepalive + reconnect logic unchanged from M1.
* No polling: the harness reads only the gRPC stream (RPC is not used).

## 3. Live Data Path

```text
SubscribeUpdate (Yellowstone)
  -> normalize()                       [src/events.rs]
     - attribute each `Program data:` line to the program on the invocation stack
     - decode events only from pump.fun / pump_amm
  -> DecodedSwap (venue, ix_name, amounts, reserves, virtual reserves, fees)
  -> swap_pre_state()                  [src/quote.rs]
     - pump.fun: event reserves are POST-trade -> pre = post - delta
     - pump.swap: event reserves are PRE-trade -> pre = event reserves
     - pump.swap effective quote = raw_quote + signed virtual_quote_reserves
  -> predict_swap() / predict_token_target_quote()
  -> exact integer comparison vs the on-chain amount in the same event
```

Every quantity is an exact integer (`u128`/`i128`, checked arithmetic). No
floating point is used for any protocol quote.

## 4. Pump.fun Results

Live 60 s run, single-event and multi-event transactions alike (each swap is
evaluated from its own event).

| Instruction | Formula | Samples | Exact | Mismatch | Errors |
|---|---|---:|---:|---:|---:|
| SELL | `floor(pre_vq·base_in/(pre_vb+base_in))` | 1402 | 1154 (82.3%) | 248 | 94 |
| `buy_exact_sol_in` / `buy_exact_quote_in` | `floor(pre_vb·(sol−1)/(pre_vq+sol−1))` | 528 | **528 (100%)** | 0 | 58 |
| `buy` (token-target) | `sol = ceil(pre_vq·tok/(pre_vb−tok))` | 1132 | 894 (79.0%) | 238 | 30 |

`pre_*` are the **virtual** (synthetic) reserves reconstructed from the event.
The `−1` on exact-in buys is live-confirmed.

## 5. PumpSwap Results

| Instruction | Formula | Samples | Exact | Mismatch | Errors |
|---|---|---:|---:|---:|---:|
| SELL | `floor(q_eff·base_in/(base+base_in))`, `q_eff = raw_quote + event virtual` | 10683 | **10683 (100%)** | 0 | 0 |
| `buy_exact_quote_in` | `floor(base·(uq−1)/(q_eff+uq−1))` | 6523 | **6523 (100%)** | 0 | 0 |
| `buy` (token-target) | `floor(base·q_in/(q_eff+q_in))` | 2435 | **2435 (100%)** | 0 | 0 |

**19,641 exact / 19,641 samples, 0 mismatches, 0 errors.** Effective quote
reserves use the **event's trade-time** `virtual_quote_reserves`, never the
account's later value (the M2.2B regression). Non-zero virtual reserves are
exercised (the `(20,5)` regime resolved in M2.2 was included).

## 6. Yellowstone-Only Assessment

| Path | Classification | Reason |
|---|---|---|
| pump.swap SELL | **YELLOWSTONE-ONLY EXACT** | event carries pre reserves, signed virtual, fees |
| pump.swap `buy_exact_quote_in` | **YELLOWSTONE-ONLY EXACT** | event carries `user_quote_amount_in` + virtual |
| pump.swap `buy` (token-target) | **YELLOWSTONE-ONLY EXACT** | pool quote inflow + reserves in the event |
| pump.fun `buy_exact_sol_in` / `buy_exact_quote_in` | **YELLOWSTONE-ONLY EXACT** | post-trade reserves + amount give pre-state |
| pump.fun SELL | **YELLOWSTONE-ONLY BUT STATE-DEPENDENT** | formula exact on account-grounded samples, but ~18% of live events' reserves are inconsistent with the trade → the bonding-curve **account** stream is required |
| pump.fun `buy` (token-target) | **YELLOWSTONE-ONLY BUT STATE-DEPENDENT** | same |
| any unknown/absent `ix_name` | **NOT RECONSTRUCTABLE** | refused (`UnsupportedInstruction`) |

Payments for pump.fun sells / token-target buys that fail the reserve
consistency check are **not** formula failures — see §9.

## 7. Exact Parity Matrix

| Protocol | Instruction | State source | Samples | Exact | Mismatch | Errors | Yellowstone-only |
|---|---|---|---:|---:|---:|---:|---|
| PumpSwap | SELL | Yellowstone event | 10683 | 10683 | 0 | 0 | EXACT |
| PumpSwap | `buy_exact_quote_in` | Yellowstone event | 6523 | 6523 | 0 | 0 | EXACT |
| PumpSwap | `buy` | Yellowstone event | 2435 | 2435 | 0 | 0 | EXACT |
| Pump.fun | `buy_exact_sol_in`/`_quote_in` | Yellowstone event | 528 | 528 | 0 | 58 | EXACT |
| Pump.fun | SELL | Yellowstone event | 1402 | 1154 | 248 | 94 | STATE-DEPENDENT |
| Pump.fun | `buy` | Yellowstone event | 1132 | 894 | 238 | 30 | STATE-DEPENDENT |

Mismatch classification (§12): the pump.fun mismatches are
`wrong trade-time state` (the event's own reserves do not satisfy
`state_before + delta = state_after` for those payloads) → `missing Yellowstone
field/state`; the `errors` are `decoder_inconsistent` (values impossible on
chain, e.g. `token_amount > 10^15` supply or a zero reserve with a non-zero
trade) → `wrong account layout / unmodeled event variant`. No mismatch was a
`formula error` or `rounding error` for any path in the matrix.

## 8. Latency / Throughput

Release build, 1 vCPU, single ingestion task (this harness does not shard; the
sharded engine is benchmarked separately).

| Stage | p50 | p95 | p99 |
|---|---:|---:|---:|
| `normalize` + decode (per update) | 9 µs | 66 µs | 143 µs |
| quote + parity check (per swap) | 0.32 µs | 0.75 µs | 1.01 µs |

Throughput: **83,711 updates / 60 s ≈ 1,395/s**, **22,885 swaps decoded / 60 s
≈ 381/s** (rate-limited by the live chain/filters, not by processing). Error
rate: 0 dropped, 0 panics; 182 coherence-rejected pump.fun events (counted, not
crashed).

## 9. Problems Found

1. **Foreign-event collision (`VERIFIED`, fixed).** Anchor event discriminators
   are `sha256("event:<Name>")[..8]` — *name*-based — so an unrelated program
   with an event called `TradeEvent` collides with pump.fun's. The normalizer now
   attributes each `Program data:` line to the program on the invocation stack
   and decodes only events from known venue programs. Before the fix, ~25% of
   "swaps" were garbage (`sol_amount = 0`, impossible reserves) — this alone
   caused most of the earlier "mismatches".
2. **pump.swap tail offsets (`VERIFIED`, fixed).** The BuyEvent `ix_name` string
   sits after `track_volume + 5×u64` (string at offset 401/405), not 4 u64; the
   appended `virtual_quote_reserves` follows `ix_name` + 4×u64. Using the wrong
   offset corrupted `ix_name`/virtual and produced the M2.2B `(20,5)` ±1
   residual. With the correct offset, SELL is 100%.
3. **SELL argument order (`VERIFIED`, fixed).** The quote prediction initially
   passed quote/base to the constant-product helper swapped; corrected to
   `floor(q_eff·base_in/(base+base_in))`.
4. **pump.swap exact-in input (`VERIFIED`, fixed).** `buy_exact_quote_in` uses
   `user_quote_amount_in` (net), not the pool quote inflow.
5. **Credential rename (`VERIFIED`, fixed earlier).** `.env` moved to
   `SOLAMI_GRPC_API_KEY` / `SOLAMI_GRPC_ENDPOINT`; the runtime now accepts the
   new names as aliases.

## 10. Changes Made

| File | Change |
|---|---|
| `src/events.rs` | program-stack attribution of `Program data:` lines; decode only venue-program events |
| `src/decode/mod.rs` | `DecodedSwap` gains `user_quote_amount`, `virtual_base_reserve` |
| `src/decode/pumpfun.rs` | decode `virtual_token_reserves` and the `ix_name` tail |
| `src/decode/pump_amm.rs` | decode `ix_name` (buy) + signed appended `virtual_quote_reserves`; capture `user_quote_amount`; tolerate short/older payloads |
| `src/quote.rs` | `Instruction`, `classify`, `SwapPreState`, `swap_pre_state`, `predict_swap`, `predict_token_target_quote`; `UnsupportedInstruction` |
| `src/validate.rs` (new) | live Yellowstone parity harness (`neurone validate`) |
| `src/main.rs` | `validate` command |
| `src/config.rs`, `src/ingest/solami.rs`, `tests/live_solami.rs` | credential-name aliases |
| `src/ingest/simulated.rs`, `tests/{reserve_state,decode_and_state}.rs` | structural updates for the new `DecodedSwap` fields |
| `tests/m3_parity.rs` (new) | deterministic regression tests (see §13) |

## 11. Regression Protection

`tests/m3_parity.rs` (8 tests, all passing) pins: pump.swap SELL exact; signed
virtual reserves raising/lowering the price; a negative effective reserve
rejected; pump.swap `buy_exact_quote_in` `−1`; pump.swap token-target buy;
pump.fun exact-in `−1`; pump.fun token-target `ceil`; strict instruction
classification. All M1/M2/M2.1 tests remain green (`cargo test`: 89 deterministic
tests, 1 ignored live test).

## 12. Final M3 Status

**`PASS WITH EXCLUSIONS`**

Empirically demonstrated on live Solami Yellowstone:

```text
Yellowstone -> live decode -> trade-time state -> exact protocol quote -> on-chain parity
```

* **PASS (100% exact, 19,641/19,641):** all three PumpSwap paths and pump.fun
  exact-in buys.
* **EXCLUDED (state-dependent):** pump.fun SELL and token-target `buy` are
  formula-correct (validated account-grounded offline) but the **event alone**
  is insufficient for ~18–21% of live events whose recorded reserves are
  inconsistent with the trade; those require the bonding-curve **account**
  stream (a Yellowstone account subscription) for the true trade-time state, or
  a version-aware `TradeEvent` decoder. They are refused rather than guessed.

No trading, Beam, TP/SL, capital allocation or strategy logic was added.
