# Neurone — M3.2A: Pump.fun Fee Arithmetic + Pre-State Gate

## Objective

Close the two concrete issues identified by M3.2:

1. Adopt the official `@pump-fun/pump-sdk` v3.0.0 fee arithmetic.
2. Add a deterministic pre-state corroboration gate for Pump.fun SELL and token-target BUY.
3. Re-run live Yellowstone parity.
4. If mismatches remain, capture transaction-local evidence before changing anything else.

This is a focused investigation/closure task. Do **not** implement trading, Beam, TP/SL, capital arbitration, or unrelated protocol work.

## Source of Truth

Use:

`MILESTONE_3_2_PUMPFUN_SELL_BUY_INVESTIGATION.md`

The M3.2 report's official SDK findings are authoritative for this task.

## 1. Fix Pump.fun Fee Arithmetic

### BUY exact-in

Use the official integer sequence:

```text
totalFeeBps =
    protocolFeeBps
    + creatorFeeBps if bondingCurve.creator != Pubkey::default

inputAmount =
    (amount - 1) * 10000
    / (totalFeeBps + 10000)
```

Do **not** use:

```text
amount - floor(amount * fee / 10000)
```

That is the known Neurone defect.

### SELL

Calculate gross output:

```text
gross =
    floor(
        virtualQuoteReserves * tokenAmount
        / (virtualTokenReserves + tokenAmount)
    )
```

Then:

```text
net = gross - fee(gross)
```

### Token-target BUY

Use:

```text
minAmount = min(amount, realTokenReserves)

solCost =
    minAmount * virtualQuoteReserves
    / (virtualTokenReserves - minAmount)
    + 1

totalCost = solCost + fee(solCost)
```

Do not replace the explicit `+1` with a generic ceiling.

## 2. Dynamic Fees

Do not hard-code a fee tier.

Use the observed/decoded protocol fee state or the existing correct `computeFeesBps` path.

Preserve the creator-fee rule:

```text
creatorFee applies only when creator != Pubkey::default
```

## 3. Pre-State Corroboration Gate

Pump.fun SELL and token-target BUY must not be classified as exact/executable unless the trade pre-state is corroborated.

Acceptable evidence includes:

- a previous event post-state forming a valid contiguous state transition, or
- appropriate bonding-curve account state from the existing Yellowstone account cache.

If the true pre-state cannot be corroborated:

```text
UnsupportedState
```

Do not guess or silently treat event post-delta state as true pre-state.

## 4. Preserve Existing Working Paths

Do not regress:

- PumpSwap SELL
- PumpSwap exact quote-in BUY
- PumpSwap token-target BUY
- Pump.fun exact-in BUY
- USDC/non-SOL quote handling
- program attribution
- Yellowstone ingestion
- bonding-curve account cache
- deterministic integer quote math
- parallel sharded state architecture

Do not introduce RPC polling into the hot path.

## 5. Live Yellowstone Parity Rerun

Run the existing live parity harness.

Report separately:

- Pump.fun SELL
- Pump.fun exact-in BUY
- Pump.fun exact-quote BUY
- Pump.fun token-target BUY
- SOL vs non-SOL where applicable

For each category report:

```text
samples
exact
mismatch
errors
unsupported_state
parity %
```

Also classify failures as:

```text
formula/fee mismatch
state-not-corroborated
decode/error
```

Do not count excluded samples as exact.

## 6. Decision Gate

### If SELL/token-target parity materially improves

Record:

- exact results
- exclusions
- supported paths
- unsupported paths
- whether Yellowstone state is sufficient

Then **stop**.

### If residual mismatches remain

Do **not** keep changing formulas.

Capture failing transactions with:

- signature
- slot
- instruction index/order
- complete relevant Pump.fun instruction sequence
- bonding-curve account updates
- TradeEvent
- event-derived state
- account-stream state
- pre/post reserves
- fee values
- ix_name
- quote mint
- layout length/version

Investigate whether the transaction contains reserve/state mutations such as:

- buyback / buy-and-burn
- holder rewards
- cashback
- `set_virtual_quote_reserves`
- account extension/update instructions
- other reserve-affecting instructions

This is evidence collection only. Do not invent a new formula.

## Acceptance Criteria

1. Official SDK fee arithmetic is implemented exactly.
2. Creator-fee gating is correct.
3. SELL/token-target BUY require corroborated pre-state.
4. Live Yellowstone parity is rerun.
5. Exact, mismatch, error, and unsupported-state are reported separately.
6. Existing PumpSwap and Pump.fun exact-in paths remain green.
7. If residual mismatches remain, failing transactions have sufficient local evidence to test the mutation hypothesis.
8. No unrelated architecture or trading features are added.

## Hard Constraints

- Rust production path.
- Integer arithmetic only for protocol quote math.
- No floating-point quote math.
- No LLM/narrative scoring.
- No strategy changes.
- No Beam.
- No TP/SL.
- No capital-arbitration changes.
- No polling replacement for Yellowstone.
- No broad refactor.
- No speculative formula changes.
- Keep changes minimal and auditable.

## Final Deliverable

Produce a concise milestone report with:

1. Files changed.
2. Fee-arithmetic changes.
3. Pre-state gate.
4. Live parity results.
5. Before/after comparison.
6. Supported paths.
7. Unsupported/excluded paths.
8. Remaining mismatches and evidence.
9. Verdict:

```text
PASS
PASS WITH EXCLUSIONS
or
BLOCKED
```

If PASS or PASS WITH EXCLUSIONS, stop. Do not automatically proceed to the next Neurone milestone.
