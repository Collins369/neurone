#!/usr/bin/env node
/**
 * Neurone — Blur SOL/USD reference probe (read-only research artifact).
 *
 * Blur is Solami's decoded market-data feed:
 *   wss://ws.solami.dev/data/subscribe?chain=solana&api_key=<DataApi key>&<filters>
 *
 * It requires an API key with the `DataApi` permission (WebSocket `?api_key=`,
 * server `x-api-key`). This probe is NOT part of the Neurone runtime and is NOT
 * wired into M4. It exists to gather the live evidence needed before any
 * integration decision could be made.
 *
 * Status at the time of writing: Neurone's available credentials lack `DataApi`
 * (WS close 1006 before upgrade; REST 403 `missing required permission:
 * DataApi`), so this probe could not be run. Provide a DataApi-scoped key in
 * SOLAMI_BLUR_KEY to run it:
 *
 *   SOLAMI_BLUR_KEY=... node research/blur/probe.mjs 120
 *
 * It filters to SOL-quoted swaps (the only way Blur could expose a SOL/USD
 * print) and prints per-event metadata plus a summary (cadence, price spread,
 * outlier rate) that can be compared against an independent external SOL/USD
 * reference used for validation only.
 */

const KEY = process.env.SOLAMI_BLUR_KEY || process.env.SOLAMI_DATA_API_KEY || "";
const SECONDS = Number(process.argv[2] || 60);
const SOL = "So11111111111111111111111111111111111111112";

if (!KEY) {
  console.error("no SOLAMI_BLUR_KEY (needs an API key with the DataApi permission)");
  process.exit(2);
}

const url =
  `wss://ws.solami.dev/data/subscribe?chain=solana&api_key=${KEY}` +
  `&type=swap&address=${SOL}&backfill=5`;

const prices = []; // { t, priceUsd, slot, blockTime, dex, pool, mint, quoteMint, impact, candleOk }
let openedAt = 0;
let frames = 0;
let lastArrival = 0;
const gaps = [];
let outliers = 0;

function pct(xs, q) {
  if (!xs.length) return null;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.floor((s.length - 1) * q))];
}

function connect() {
  const ws = new WebSocket(url);
  ws.onopen = () => {
    openedAt = Date.now();
    console.log(`[open] ${new Date(openedAt).toISOString()}`);
  };
  ws.onmessage = (e) => {
    const now = Date.now();
    let ev;
    try {
      ev = JSON.parse(String(e.data));
    } catch {
      console.log("[malformed] ignoring unparseable frame");
      return;
    }
    if (ev.type === "connected") {
      console.log("[connected frame]", JSON.stringify(ev));
      return;
    }
    if (ev.type !== "swap") return;
    frames += 1;
    if (lastArrival) gaps.push(now - lastArrival);
    lastArrival = now;

    const priceUsd = Number(ev.price_usd);
    const impact = Number(ev.price_impact_pct);
    // Outlier heuristic for reporting only: >5% single-print move vs the
    // running median, or the vendor's own candle guard flag.
    const med = prices.length ? pct(prices.map((p) => p.priceUsd), 0.5) : priceUsd;
    const isOutlier =
      ev.candle_ok === false ||
      (med > 0 && Math.abs(priceUsd - med) / med > 0.05);
    if (isOutlier) outliers += 1;

    prices.push({
      t: now,
      priceUsd,
      slot: ev.slot,
      blockTime: ev.block_time,
      dex: ev.dex,
      pool: ev.pool,
      mint: ev.mint,
      quoteMint: ev.quote_mint,
      impact: Number.isFinite(impact) ? impact : null,
      candleOk: ev.candle_ok,
    });
    console.log(
      `[swap] slot=${ev.slot} block_time=${ev.block_time} dex=${ev.dex} ` +
        `mint=${(ev.mint || "").slice(0, 6)}.. quote=${(ev.quote_mint || "").slice(0, 6)}.. ` +
        `price_usd=${ev.price_usd} impact=${ev.price_impact_pct} candle_ok=${ev.candle_ok}`,
    );
  };
  ws.onerror = () => {};
  ws.onclose = (e) => {
    console.log(`[close] code=${e.code} reason=${JSON.stringify(e.reason)}`);
    // The environment is a live stream; reconnect with backoff unless done.
    if (Date.now() - openedAt < SECONDS * 1000) setTimeout(connect, 1000);
  };
  return ws;
}

const ws = connect();

function summary() {
  const ps = prices.map((p) => p.priceUsd).filter((x) => Number.isFinite(x) && x > 0);
  console.log("\n=== Blur SOL/USD probe summary ===");
  console.log("swap frames:", frames, "unique pools:", new Set(prices.map((p) => p.pool)).size);
  console.log(
    "update cadence ms: p50=", pct(gaps, 0.5),
    "p95=", pct(gaps, 0.95),
    "max=", gaps.length ? Math.max(...gaps) : null,
  );
  console.log(
    "price_usd: n=", ps.length,
    "min=", ps.length ? Math.min(...ps) : null,
    "p50=", pct(ps, 0.5),
    "max=", ps.length ? Math.max(...ps) : null,
  );
  console.log("outlier prints (vendor flag or >5% vs median):", outliers);
  console.log(
    "NOTE: compare this against an independent external SOL/USD reference used ONLY for validation.",
  );
}

process.on("SIGINT", () => {
  try { ws.close(); } catch {}
  summary();
  process.exit(0);
});

setTimeout(() => {
  try { ws.close(); } catch {}
  summary();
  process.exit(0);
}, SECONDS * 1000);
