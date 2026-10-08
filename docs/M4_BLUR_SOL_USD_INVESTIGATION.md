# Neurone — Blur SOL/USD Reference Investigation

Goal: determine whether Solami's **Blur** market-data WebSocket can provide a
trustworthy, low-latency **SOL/USD** reference for Neurone's M4 USD valuation,
and integrate it **only if proven**.

Result: **OUTCOME C — PROMISING BUT NOT PROVEN.** Blur is documented to expose a
per-trade `price_usd` and can in principle yield a SOL/USD print from a
SOL-quoted market, but the live evidence required to trust it could **not be
obtained** (the available credentials lack the `DataApi` permission). No Blur
integration was implemented. The static operator price remains. Yellowstone
remains the primary market-data path.

---

## 1. Exact Blur API/feed investigated

Solami's docs are client-rendered; the authoritative text lives in the site
bundle (`https://solami.dev/assets/index-*.js`), read here in full (same method
as `docs/SOLAMI_RESEARCH.md`). Findings:

| Surface | Endpoint | Auth |
|---|---|---|
| **Blur data (WebSocket)** | `wss://ws.solami.dev/data/subscribe` (regional `fra.ws.solami.dev`, …) | API key on `?api_key=` (browser) or `x-api-key` (server), **`DataApi` permission** |
| Blur REST | `https://api.solami.dev/data/*` (`/pool`, `/pools`, `/pool/tvl`, `/token`, `/pnl/*`, …) | `DataApi` (`x-api-key` / `Bearer`) |
| Blur gRPC | same decoded pipeline | `DataApi` |

Explicitly distinguished from the other Solami surfaces:

* **Yellowstone (Geyser) gRPC** — raw `SubscribeUpdate` frames; Neurone's primary path.
* **Mirage** — the *same* Geyser frames over a plain WebSocket.
* **`wss://ws.solami.dev/ws/sol`** — the **standard Solana RPC WebSocket**
  (`accountSubscribe`/`programSubscribe`/`slotSubscribe`/…), **not** a price feed.
* **`GET https://api.solami.dev/pricing`** — billing REST; `llms.txt` calls it a
  live pricing object "including the SOL / USD reference".
* Blur — decoded market/token data (trades, liquidity, launches, pools, prices,
  candles, stats) with USD already computed.

## 2. Exact event/filter used

```
wss://ws.solami.dev/data/subscribe?chain=solana&api_key=<DataApi key>
    &type=swap&address=<mint>&backfill=5
```

* Filters are the query string; **within a field OR, across fields AND**. Fields
  include `type` (`swap, liquidity, token_create, pool_create, transfer, candle,
  meme, surge, graduation, …` — twelve types), `address` (mints — **"SOL / USDC /
  USDT / USD1 work too"**), `pool`, `trader`, `dex`, `side`,
  `min_volume_usd/min_base/min_quote`, `min/max_progress`, `min_multiple`,
  `min/max_mcap_at_trigger`, and `backfill` (last N events on connect, up to
  200). An empty filter is the firehose.
* First frame: `{"type":"connected","region":"fra","filter":{...}}`.
* `swap` schema (from the docs):

```json
{ "type":"swap","signature":"…","slot":0,"block_time":0,"dex":"pumpswap",
  "pool":"…","mint":"…","quote_mint":"So111...112","trader":"…","side":"buy",
  "base_amount":0,"quote_amount":0,"base_decimals":6,"quote_decimals":9,
  "base_reserve":0,"quote_reserve":0,"fee_amount":0,"fee_mint":"So111...112",
  "fee_pct":"0","price_impact_pct":"0","price":"0","price_usd":"0",
  "volume_usd":"0","candle_ok":true }
```

Numbers that are floats arrive as **decimal strings**; integers as JSON numbers.

## 3. Whether SOL/USD is actually available

* There is **no dedicated SOL/USD field**. `price_usd` is the **traded token's**
  USD price for that swap.
* SOL/USD could only be **inferred** from a swap on a **SOL-quoted** market
  (e.g. a SOL/USDC pool): the SOL leg's `price_usd` is SOL priced in USD. Blur
  decodes "every major DEX", and the `address` filter accepts SOL/USDC mints, so
  such markets plausibly exist — but this was **not verifiable live** (§5).
* It would be a **per-trade print from one pool**, not a mid or an oracle.

## 4. How Blur derives/provides the USD price

From the docs: Blur computes USD from the quote asset, and **"USD needs the SOL
price at that slot, not now"** — i.e. Blur itself uses a SOL price internally to
value SOL-quoted trades. The docs also warn explicitly:

> "One bad print — slippage, a partial fill, a mis-decoded leg — and your
> candle's high is a lie. **You need an outlier guard.**"

and define `candle_ok: false` when **Blur's own per-token price guard judged the
print an outlier** (the raw swap still streams; it just doesn't touch a candle).
So the vendor documents that individual prints can be wrong and that consumers
must guard against outliers. No aggregation method, source-pool set, or
manipulation guard is documented for the `price_usd` field itself.

## 5. Live event samples/statistics

**None — access was blocked.** Empirically (both available keys):

```
WS  wss://ws.solami.dev/data/subscribe?chain=solana&api_key=<key>            -> close 1006, opened=false
WS  ...&type=swap&address=So111…112                                        -> close 1006, opened=false
REST GET https://api.solami.dev/data/pools?chain=solana&limit=1            -> 403 {"message":"missing required permission: DataApi","required_permission":"DataApi"}
REST (same, with the RPC key)                                              -> 403 same
```

Close **1006 before the upgrade** is exactly the documented "bad key" behavior.
Neurone's `.env` holds only `SOLAMI_GRPC_API_KEY` and `SOLAMI_RPC_API_KEY`;
neither carries `DataApi`.

## 6. Update frequency

**Unknown (not measured).** Cannot be measured without a `DataApi` key.

## 7. End-to-end latency

**Unknown (not measured).** Blur is documented as "decoded events as they
confirm … no lag beyond block time", but no latency was measured. The probe
(§12/§16) is designed to measure event→local-update latency, p50/p95/p99.

## 8. Freshness behavior

**Unknown.** Blur events carry `slot` and `block_time` (so freshness *can* be
tracked), but no documented heartbeat/cadence for `/data/subscribe`, so a
freshness bound could not be derived from observation. Per the task rule, no
timeout was invented.

## 9. Reconnect behavior

**Unknown for Blur.** Documented WebSocket disconnect codes (`4029` = max
concurrent connections for the tier, `4002` = bandwidth/balance) are described
for the standard `/ws/sol` socket; for Blur, a bad key is rejected before the
upgrade (1006), as observed.

## 10. Outlier / manipulation analysis

**Not measured (no data).** Two documented, disqualifying-on-paper points stand:
(a) Blur's own docs say a single bad print can corrupt a USD value and that an
outlier guard is required, and `candle_ok` exists precisely because individual
prints are sometimes outliers; (b) a SOL/USD value inferred from one pool's
trade print is a **single-market** figure. Both would need live measurement
before they could drive a $2k/$10k boundary (a 5% price error is 5% on every USD
gate).

## 11. Independent-reference comparison

**Not performed** — there was no Blur SOL/USD data to compare. An independent
external SOL/USD reference remains validation-only and must never become a
runtime dependency.

## 12. Exact architecture implemented, if any

**None.** No production code was changed. No Blur client, no reference state, no
config, no telemetry. Artifacts added (research only):

```
research/blur/probe.mjs    dependency-free Node 22 Blur /data/subscribe probe
                           (filters to SOL-quoted swaps; prints slot, block_time,
                           dex, pool, mint, quote_mint, price_usd,
                           price_impact_pct, candle_ok + a summary)
research/blur/README.md    how to run it and what evidence is still required
```

Nothing under `src/` or `config/` was touched for Blur.

## 13. Why it is (not) safe for M4

Not proven safe to consume. Blur documents `price_usd`, but for SOL/USD it is a
single-pool, per-trade USD print whose derivation/aggregation is undocumented and
which the vendor itself says needs an outlier guard. Without live measurements we
cannot establish availability, cadence, freshness, latency, or manipulation
resistance — so it must not drive the `$2,000` liquidity floor, the
`$2,000–$10,000` MCAP band, or the `$1,000` rolling-volume floor. M4 keeps the
operator-supplied static price and stays inactive until it is set (fail closed).

## 14. Performance before/after

Unchanged: no integration, so no hot-path impact. Blur would be — if ever
authorized — a background async receiver updating an atomic/reference state that
M4 reads locally; M4 `evaluate()` must never perform network I/O. Current M4
benchmarks are unchanged (≈250k ev/s with M4 active; `ns_per_eval ≈ 223`).

## 15. Complete test results

```
cargo test --all-targets        181 passed, 0 failed, 1 ignored
cargo fmt --check               clean
cargo clippy --all-targets --all-features -- -D warnings   clean
cargo build --release --locked  ok
cargo test --test live_solami --locked -- --ignored --nocapture   1 passed
```

(The single ignored test is `live_authenticated_stream_decodes_markets`, kept
`#[ignore]` per `docs/M4_IGNORED_TEST_AUDIT.md`.)

## 16. Remaining limitations

1. **No `DataApi` credential** → Blur cannot be probed or consumed at all today.
2. No documented SOL/USD feed: SOL/USD must be inferred from a SOL-quoted pool's
   swap print (single market).
3. `price_usd` is a per-trade print with a vendor-documented outlier risk; no
   aggregation/manipulation guard is documented for the field.
4. No measured cadence/latency/freshness; no derived freshness bound.
5. The live pricing REST (`api.solami.dev/pricing`) currently exposes **no**
   SOL/USD field (only pricing + `payg.blur.usd_per_gb=0.20`,
   `bandwidth_weight=2.0`), so it is not a reference source.

## 17. Yellowstone remains the primary market-data path

Confirmed. Yellowstone gRPC remains the sole source for token discovery, market
state, strategy inputs, and all trading events. `/ws/sol` is the standard Solana
RPC WebSocket; Mirage is raw Geyser over WebSocket; neither is used. The M4
strategy still uses only Yellowstone-derived state. No HTTP/RPC polling was
added.

## 18. No M5/M6 functionality introduced

Confirmed. No arming, pre-arming, triggers, transaction construction, signing,
Beam, capital arbitration, positions, TP/SL, exits, P&L, LLM/narrative logic,
social scoring, database hot-path writes, or new DEX integrations were added.
M4 still stops at `OBSERVING → QUALIFIED`, with thresholds unchanged
(`$2,000` liquidity, `$1,000` rolling 5m volume, `$2,000 ≤ MCAP ≤ $10,000`,
900 s max age) and `max_drawdown_bps` still unset/`None`.

---

## Decision

`OUTCOME C — PROMISING BUT NOT PROVEN`

Blur is documented to expose `price_usd` on `/data/subscribe`, and a SOL/USD
print is plausible from a SOL-quoted market — but the live evidence required to
trust it could not be obtained (no `DataApi` key), and the field is a single-pool
trade print that the vendor itself says needs an outlier guard. **Do not
integrate.** Keep the static operator price. To continue, obtain a
`DataApi`-scoped key, run `research/blur/probe.mjs` for several minutes, and
compare against an independent external SOL/USD reference (validation only) with
manipulation and staleness tests.
