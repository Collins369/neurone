# Neurone — M3.2A: Pump.fun Fee Arithmetic + Pre-State Gate

Focused closure task. No trading/execution. Integer math only.

Statuses: `CONFIRMED`, `OBSERVED`, `INFERRED`, `HYPOTHESIS`, `UNRESOLVED`.

---

## 1. Files Changed

| File | Change |
|---|---|
| `src/quote.rs` | Added the official SDK fee primitives (`fee_ceil`, `buy_input_from_gross`, `token_target_sol_cost`, `sell_net_from_gross`) and `QuoteError::UnsupportedState`. No change to the validated event-level predictors. |
| `src/validate.rs` | Pre-state corroboration gate for pump.fun SELL / token-target BUY; per-path `unsupported_state` counter and examples; contiguity from the previous event + account-cache cross-check. |
| `tests/m3_parity.rs` | 4 new deterministic tests for the official fee arithmetic (ceil fee, `(amount−1)·10000/(bps+10000)`, token-target `floor+1`, gated creator fee). |

Nothing else changed; the sharded engine, ingestion, decoders and PumpSwap paths
are untouched.

## 2. Fee-Arithmetic Changes

The official `@pump-fun/pump-sdk` v3.0.0 arithmetic was implemented **exactly**
as client-side quote primitives:

```
fee(amount, bps)       = ceil(amount * bps / 10000)
buy input from gross   = (amount - 1) * 10000 / (totalFeeBps + 10000)   // floor
token-target sol cost  = x * vq / (vt - x) + 1                          // floor, explicit +1
sell net               = gross - fee(gross, protocol) - fee(gross, creator)
totalFeeBps            = protocol + creator   (creator only when creator != default)
```

`tests/m3_parity.rs::official_*` pin each rule (`fee_ceil(987653, 95) = 9383`,
`buy_input_from_gross(1_000_000, 125) = 987653`, `token_target_sol_cost =
floor+1`, gated creator subtraction).

**Correction to the M3.2 proposal (`OBSERVED`).** The SDK's
`(amount−1)·10000/(feeBps+10000)` maps a **user-specified gross** amount to a
curve input. A `TradeEvent` already records **curve-level** values, so applying
that inversion to event data *regresses* the currently-perfect path:

| Predictor on 92 real `buy_exact_sol_in` events | Exact |
|---|---:|
| `floor(vt·(sol−1)/(vq+sol−1))` (event-level, current) | **92/92** |
| `floor(vt·input/(vq+input))` with `input = (sol−1)·10000/(bps+10000)` (SDK inversion) | **0/92** |

The SDK function is therefore implemented for **client-side quoting from a user
amount**; the event-level predictor stays on curve-level values. The M3.2 claim
that fee arithmetic alone explains the residual is **disproven**.

## 3. Pre-State Gate

Pump.fun SELL and token-target BUY are evaluated only when the trade's
pre-state is **corroborated** by independent evidence:

* **contiguity** — the previous observed event on the same curve has
  `slot < trade.slot` and `post == derived pre`; or
* **account state** — the bonding-curve account cache holds a state strictly
  older than the trade's slot equal to the derived pre.

Otherwise the trade returns `QuoteError::UnsupportedState` and is counted
separately — never guessed, never counted as exact. Pump.fun exact-in buys and
all PumpSwap paths do not need the gate.

## 4. Live Parity Results

Live Solami Yellowstone, release, 60 s: 82,717 updates, 19,645 swaps,
3,555 curve-account updates, 271 curves cached; 3,603 correlations,
1,556 corroborated.

| Protocol | Instruction | Samples | Exact | Mismatch | Errors | Unsupported | Parity |
|---|---|---:|---:|---:|---:|---:|---:|
| PumpSwap | SELL | 9,483 | 9,483 | 0 | 0 | 0 | 100% |
| PumpSwap | `buy_exact_quote_in` | 4,314 | 4,314 | 0 | 0 | 0 | 100% |
| PumpSwap | `buy` | 2,245 | 2,245 | 0 | 0 | 0 | 100% |
| Pump.fun | `buy_exact_sol_in`/`_quote_in` | 757 | 757 | 0 | 0 | 0 | 100% |
| Pump.fun | SELL | 747 | 747 | 0 | 0 | 893 | 100% of supported |
| Pump.fun | token-target `buy` | 445 | 445 | 0 | 0 | 761 | 100% of supported |

**Every supported sample is exact (17,991/17,991).** The only non-exact bucket
is `unsupported_state` (1,654), excluded by design — not a mismatch.

Failure classification: `state-not-corroborated` = 1,654;
`formula/fee mismatch` = 0; `decode/error` = 0.

## 5. Before / After Comparison

| Path | M3.1 (before) | M3.2A (after) |
|---|---|---|
| Pump.fun SELL | 1,131/1,406 (80.4%), 275 mismatch | 747/747 (100%), 893 unsupported |
| Pump.fun token-target `buy` | 709/994 (71.3%), 285 mismatch | 445/445 (100%), 761 unsupported |
| Pump.fun `buy_exact_*` | 647/647 (100%) | 757/757 (100%) |
| PumpSwap SELL / `buy_exact_quote_in` / `buy` | 100% | 100% |

The previously-reported mismatches disappear once non-corroborated pre-states
are excluded instead of evaluated — confirming the M3.2 root cause:
**pre-state availability**, not the quote formula or the fee arithmetic.

## 6. Supported Paths

* PumpSwap SELL, `buy_exact_quote_in`, `buy` — Yellowstone-only exact.
* Pump.fun `buy_exact_sol_in`, `buy_exact_quote_in` — Yellowstone-only exact.
* Pump.fun SELL, token-target `buy` — exact **when the pre-state is
  corroborated** (contiguous previous event or bonding-curve account state).

## 7. Unsupported / Excluded Paths

* Pump.fun SELL / token-target `buy` whose pre-state cannot be corroborated →
  `UnsupportedState`: curves with no contiguous previous event observed and no
  matching older cached account state (sparse history, or a curve mutated
  outside a `TradeEvent`).

## 8. Remaining Mismatches and Evidence

Zero formula mismatches remain on supported samples. The 1,654 excluded samples
carry `example_unsupported` records in the harness. Per the decision gate, the
failing transactions would need transaction-local capture (instruction order;
buyback / holder-reward / cashback / `set_virtual_quote_reserves`) to prove
whether an intermediate mutation explains them; that was not required here
because no formula mismatch remains and the exclusion boundary is deterministic.

## 9. Verdict

**`PASS WITH EXCLUSIONS`**

* Official SDK fee arithmetic implemented exactly as client-side primitives;
  creator-fee gating correct; the SDK inversion is not applied to event-level
  prediction (it would regress 92/92 → 0/92).
* All supported paths — PumpSwap (three), pump.fun exact-in buys, and pump.fun
  SELL / token-target under the corroboration gate — are **100% exact live**
  (17,991/17,991).
* Uncorroborated pump.fun SELL / token-target trades are deterministically
  excluded as `UnsupportedState`, never guessed.
* Existing PumpSwap and pump.fun exact-in paths remain green; 93 deterministic
  tests pass; clippy/fmt clean; release build ok.

Stopping here per the decision gate; not proceeding to the next milestone.
