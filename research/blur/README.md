# Blur SOL/USD investigation artifacts

Read-only research for the M4 SOL/USD question. **Nothing here is part of the
Neurone runtime**, and **no Blur integration exists** (see
`docs/M4_BLUR_SOL_USD_INVESTIGATION.md`).

## What is in here

* `probe.mjs` — a dependency-free Node 22 client for Solami Blur's decoded
  market-data WebSocket (`wss://ws.solami.dev/data/subscribe`). It filters to
  SOL-quoted swaps and prints per-event metadata (`slot`, `block_time`, `dex`,
  `pool`, `mint`, `quote_mint`, `price_usd`, `price_impact_pct`, `candle_ok`)
  plus a summary (update cadence, `price_usd` spread, outlier rate). Use it to
  compare Blur's SOL/USD print against an independent external reference used
  **only for validation**.

## How to run

```bash
# Requires an API key with the DataApi permission; Neurone's current keys do not have it.
SOLAMI_BLUR_KEY=... node research/blur/probe.mjs 120
```

## Current status (blocked)

Blur requires the `DataApi` permission. With the credentials available to
Neurone:

* WebSocket: rejected before the HTTP upgrade — close code **1006** (the
  documented "bad key" behavior), for both filtered and unfiltered URLs.
* REST: `403 {"message":"missing required permission: DataApi","required_permission":"DataApi"}`.

So the probe could not be run, and the live evidence needed for an integration
decision (update frequency, latency, freshness, outlier behavior, independent
agreement) could not be gathered. Blur is a metered product
(`payg.blur.usd_per_gb = $0.20`, `bandwidth_weight = 2.0`).

## What is required to continue

1. A Solami API key scoped to `DataApi`.
2. Run `probe.mjs` for a meaningful period (>= a few minutes) covering active and
   quiet SOL markets.
3. Compare against >= 1 independent external SOL/USD reference (validation only,
   never a runtime dependency): median/p95/max deviation, staleness, outliers.
4. Test manipulation: can a single trade move the price Blur would give us for
   SOL, and by how much?

Only then can OUTCOME A/B/C be decided.
