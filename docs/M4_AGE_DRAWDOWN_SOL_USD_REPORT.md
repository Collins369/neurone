# Neurone — M4 Age / Drawdown / SOL-USD Report

Change set: lock the M4 maximum token age, clarify and harden the drawdown
qualification gate, and investigate a Yellowstone-derived SOL/USD reference.
No M5/M6 functionality was introduced.

---

## 1. Files changed

```
src/strategy.rs        max_token_age_seconds default = 900; slot-exact age gate;
                       max_age_slots(); overflow-safe drawdown; semantics docs
src/market.rs          unit test proving MCAP peak is a running maximum
src/reference.rs       (new) experimental SOL/USD derivation from pool reserves
                       (integer-only; NOT wired into M4)
src/telemetry.rs       split age rejections: rejected_too_old vs
                       rejected_insufficient_freshness (+ report line)
src/lib.rs             pub mod reference
config/default.toml    max_token_age_seconds = 900 (locked)
tests/strategy.rs      +21 tests (age boundary, drawdown, engine freshness)
research/solusd/probe.py  (new) read-only SOL/USDC + manipulation probe
```

No change to `src/quote.rs`, `src/decode/*`, ingestion/sharding, or the M3
validator.

---

## 2. Exact age semantics

* `max_token_age_seconds = 900` (15 minutes) — locked, and now the **default** in
  both `config/default.toml` and `StrategyConfig::default()`.
* This is a **maximum**: a market qualifies only if its chain-proven creation
  time is at most 900 s old. It is *not* a minimum lifetime and *not* a "wait 15
  minutes" rule. A 2-minute-old token with ≥$1000 rolling 5m volume remains
  eligible.
* Chain-proven creation = `created_at_slot`, set only when a chain event proves
  it (an observed pump.fun `CreateEvent`, i.e. the create transaction's slot). It
  is never inferred from first observation.
* **Boundary is slot-exact** (this fixed a real edge bug). Age is measured in
  slots and compared to `max_age_slots = ceil(900 × 2.5) = 2250`:
  `age_slots <= 2250` passes (inclusive), `2251` (15 m + 1 slot) fails. The
  previous seconds-flooring implementation would have admitted 2251 slots
  (~900.4 s) — that is now impossible.
* Unknown creation fails closed (`INSUFFICIENT_FRESHNESS_DATA`) while the gate is
  enabled — i.e. late-discovered markets do not qualify.

## 3. Exact drawdown semantics

The drawdown gate is a **market qualification / anti-pump-retracement filter**,
NOT a trade stop-loss and NOT TP/SL:

> Has this token already declined substantially from the highest MCAP Neurone
> has observed for this market?

Example: `$3k → $9k → $6k` is a 33.3 % drawdown and would fail a 20 % bound even
though $6k is still inside the $2k–$10k MCAP range. Position stop-loss belongs to
the later position/exit stages and is not implemented here.

The threshold stays **unset by default** (`max_drawdown_bps = None`) because the
project has not selected a value; the gate is disabled unless configured. No
arbitrary number was invented.

## 4. Exact formula

Integer arithmetic only, no floats:

```text
drawdown_bps = ((peak_mcap - current_mcap) * 10_000) / peak_mcap
```

* `peak_mcap = MarketState.peak_mcap_quote` (running max of `market_cap_quote()`).
* `current_mcap = market_cap_quote()` = `reference_price × token_total_supply` in
  raw quote units (deterministic).
* `current = min(current_mcap, peak_mcap)` so `current >= peak` yields 0 (a new
  high is never a drawdown).
* Truncating integer division; the threshold is inclusive (`drawdown_bps >
  max_drawdown_bps` rejects).
* `checked_mul`: overflow fails closed.

## 5. Why peak tracking is correct

`peak_mcap_quote` is a **running maximum** updated by `note_mcap_peak()` inside
every reserve/price-affecting apply (`apply_decoded_account`, `apply_swap`,
`apply_create`, `apply_vault_balances`). The shard applies the event **before**
calling `evaluate`, so at evaluation time `peak_mcap_quote = max(all observed
mcaps, current)`.

Consequences (proved by `market::tests::mcap_peak_is_a_running_maximum`):

* a new high sets `peak = current` → `drawdown = 0` (correct: no retracement);
* a retracement leaves `peak` unchanged → `drawdown > 0` (correct: the true
  decline from the observed high).

The "peak updated before drawdown ⇒ drawdown artificially zero" bug **does not
apply**: `min(current, peak)` means the only way to get drawdown 0 is for the
current value to *be* the running maximum, which is exactly zero retracement. The
peak is never lowered, so a later retracement is always measurable.

## 6. SOL/USD source investigation

Question: can Neurone derive a trustworthy SOL/USD reference from
Yellowstone-observed on-chain market data, with no HTTP/RPC polling and no
external price API on the hot path?

Method: enumerate the venues Neurone **already subscribes to** and check for a
SOL/USDC market; quantify single-trade price impact as a manipulation proxy. Raw
Yellowstone stays the only data source considered; no Solami market-data product,
no external API, no RPC in the hot path (read-only RPC used only for this offline
research).

## 7. Which on-chain markets were tested

* pump.fun bonding curves — the quote asset is SOL or USDC **per token**, base is
  the token, so no SOL/USDC pair exists by construction.
* pump.swap (`pump_amm`) pools — sampled **1,000** pools via
  `getProgramAccountsV2`; mint classes:

```text
base=SOL: 763   quote=SOL: 224   quote=USDC: 5   other: 8
SOL/USDC markets found: 0
```

Because a token's pump_amm pool carries a single quote asset, cross-deriving
SOL/USD by triangulation would require one token with *both* a SOL-quoted and a
USDC-quoted market; the sample contains no such usable, liquid pair.

## 8. Why the available reference is not trustworthy

No SOL/USDC market exists in the observable venues, so there is nothing to derive
SOL/USD from. Any substitute (triangulating thin token pools, or subscribing an
external DEX such as Raydium/Orca) would either add new protocol support beyond
this task's scope or rest on pools a single trade can move materially — and even
a large SOL/USDC pool is a single source without a multi-market median.

## 9. Live test results

`research/solusd/probe.py` (read-only RPC, offline research):

```text
pump_amm pools sampled: 1000
SOL/USDC markets found: 0
```

Live `neurone run` (45 s, Solami, default config + operator price $200/SOL,
15-minute age gate enabled):

```text
markets_evaluated            101,166
markets_qualified                 49
rejected_insufficient_freshness  86,412
rejected_too_old                   0
rejected_low_5m_volume          1,486
rejected_mcap                     950
rejected_low_liquidity             36
rejected_state                    385
strategy_eval p50               100 ns
```

The age gate behaves as specified live: markets with chain-proven creation
(launched during the run) evaluate through the other gates and 49 newly
qualified; late-discovered markets fail closed on unknown creation. There is no
runtime SOL/USD reference update to report because none is wired (see §12).

## 10. Comparison against an external reference

Not performed, and honestly so: there is **no derived reference to compare**,
because no in-scope SOL/USDC source exists. An external SOL/USD value would only
ever have been a validation aid; it must not become a runtime dependency, and it
is not one. No external value was fetched or embedded in Neurone.

## 11. Manipulation / outlier analysis

Since a single-pool reference is the only conceivable in-scope source, we
quantified how much one trade moves a real pump_amm pool price (24 real trades):

```text
single-trade price impact (bps): median 80.6 · p90 1658 · max 3365
share of trades moving price > 1%: 50%
```

Half of individual trades move the pool price by more than 1 %, and one trade
moved it ~34 %. A SOL/USD reference derived from a single such market would be
trivially manipulable and would materially shift the M4 gates (a 1 % price error
is ~1 % on every USD threshold). A trustworthy design would need multiple
independent sources with a deterministic median plus freshness/deviation
bounds — which cannot be validated without at least one suitable source. No such
safeguards were added because no source exists to justify them.

## 12. Final architecture decision for SOL/USD

**OUTCOME B — Yellowstone-derived SOL/USD is not sufficiently trustworthy with
the tested approach.** The currently subscribed venues contain no SOL/USDC
market, and single-source substitutes are manipulation-prone.

Decision:

* Keep the operator-supplied static `sol_usd_price_micros`. M4 stays inactive
  until it is set (fail closed; no invented price), exactly as before.
* Do **not** add an external price API, HTTP/RPC polling, or a new DEX
  subscription on the hot path.
* Keep the derivation as an isolated, integer-only prototype (`src/reference.rs`,
  with tests) that is **not wired into M4**, so a future authorized investigation
  resumes from a proven base.

## 13. Performance before/after

`bench` (200,000 events, 8 shards), M4 active vs inactive:

| Metric | M4 active | M4 inactive |
|---|---:|---:|
| throughput (events/s) | 250,194 | 283,112 |
| state updates | 318,405 | 318,405 |

Strategy microbench: `ns_per_eval ≈ 223` (p50 206 ns, p95 263 ns, p99 345 ns);
rolling-volume update ≈ 636 ns. The age gate adds one saturating subtract and
compare; the drawdown gate adds one `checked_mul`/divide and runs only when
configured (default off). There is **no** SOL/USD reference cost on the hot path
(not integrated). Evaluation remains allocation-free and shard-local.

## 14. Test results

`cargo test --all-targets` → **181 passed, 0 failed, 1 ignored** (was 160; +21).

New/updated deterministic tests:

* **Age**: default is 900 s; `max_age_slots() == 2250`; 0 age; very young;
  2-minute token passes; exactly 15 minutes passes (inclusive); 15 m + 1 slot
  rejects; old market rejects; unknown creation fails closed and is not inferred;
  repeated evaluation deterministic; engine-level proven-creation qualifies and
  late-without-creation fails closed.
* **Drawdown**: no drawdown at a new high; exact threshold inclusive;
  threshold+1 unit rejects; current above peak → 0; zero peak fails closed;
  `u128` overflow fails closed; `$3k→$9k→$6k` detected; deterministic repeated
  evaluation; unit test proving the peak is a running maximum (never lowered).
* **SOL/USD prototype**: exact integer derivation; zero reserves fail closed;
  overflow fails closed; freshness/`is_fresh` boundaries.
* All prior M4, M3.3, late-market, PumpSwap-bootstrap, parity and fixture tests
  remain green.

`cargo fmt --check` clean; `cargo clippy --all-targets --all-features --
-D warnings` clean; `cargo build --release --locked` ok; `live_solami` passed;
M3 parity unchanged (0 mismatches).

## 15. Remaining limitations

1. **USD valuation is still a static operator price** (`sol_usd_price_micros`).
   No trustworthy Yellowstone-only SOL/USD reference was proven; M4 is inactive
   until the operator sets the price.
2. **Freshness requires observed creation.** With the 15-minute gate enabled by
   default, markets whose launch was not observed fail closed
   (`INSUFFICIENT_FRESHNESS_DATA`) — a deliberate fail-closed consequence of "do
   not equate first observation with creation". In live runs this is the dominant
   rejection.
3. **Anti-pump needs full-history extrema.** When `max_drawdown_bps` is set,
   markets without proven creation / an established peak fail closed; the peak is
   only as good as the observations Neurone has seen (a pre-observation pump is
   invisible — hence the fail-closed requirement).
4. **Drawdown threshold is unset** by default (no project value chosen).
5. Mint/freeze authority and Token-2022 extensions remain unobservable.
6. The SOL/USD derivation prototype (`src/reference.rs`) is intentionally unused
   at runtime and cannot be validated end-to-end without an authorized source.

## 16. No M5/M6 functionality introduced

No arming, pre-arming, triggers, transaction construction, signing, Beam
submission, capital arbitration, positions, TP/SL, exits, P&L, LLM/narrative
analysis, social scoring, HTTP/RPC polling, database hot-path writes, frontend,
ShredDirect, or dynamic token-account firehose was added. M4 still stops at
`OBSERVING → QUALIFIED`.
