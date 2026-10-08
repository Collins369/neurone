#!/usr/bin/env python3
"""SOL/USD reference investigation (read-only) for M4.

1. Confirms whether any SOL/USDC market exists among the venues Neurone
   subscribes to (pump.fun bonding curves, pump.swap `pump_amm` pools).
2. Quantifies how much a *single* observed pump_amm trade moves a pool price,
   as a manipulation proxy for a would-be single-pool SOL/USD reference.

Run: SOLAMI_RPC_API_KEY=... python3 research/solusd/probe.py
"""
import base64
import collections
import json
import os
import statistics
import struct
import urllib.request

URL = f"https://rpc.solami.dev/sol?api_key={os.environ['SOLAMI_RPC_API_KEY']}"
PAMM = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"
POOL_DISC = bytes([241, 154, 109, 4, 17, 177, 109, 188])
BUY_DISC = bytes([103, 244, 82, 31, 44, 245, 119, 119])
SELL_DISC = bytes([62, 47, 55, 10, 165, 3, 220, 42])
SOL = "So11111111111111111111111111111111111111112"
USDC = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"

ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58e(b):
    n = int.from_bytes(b, "big")
    out = ""
    while n > 0:
        n, r = divmod(n, 58)
        out = ALPHABET[r] + out
    return "1" * (len(b) - len(b.lstrip(b"\x00"))) + (out or "")


def rpc(method, params, tries=4):
    for _ in range(tries):
        try:
            body = json.dumps(
                {"jsonrpc": "2.0", "id": 1, "method": method, "params": params}
            ).encode()
            req = urllib.request.Request(
                URL, data=body, headers={"Content-Type": "application/json"}
            )
            with urllib.request.urlopen(req, timeout=90) as r:
                out = json.load(r)
            if "error" in out:
                continue
            return out
        except Exception:  # noqa: BLE001
            continue
    return None


def pools(limit=1000):
    r = rpc(
        "getProgramAccountsV2",
        [
            PAMM,
            {
                "encoding": "base64",
                "filters": [{"memcmp": {"offset": 0, "bytes": b58e(POOL_DISC)}}],
                "limit": limit,
            },
        ],
    )
    out = []
    for e in r["result"]["value"]["accounts"]:
        d = base64.b64decode(e["account"]["data"][0])
        o = 8 + 1 + 2 + 32
        bm = b58e(d[o : o + 32])
        o += 32
        qm = b58e(d[o : o + 32])
        out.append((e["pubkey"], bm, qm))
    return out


def program_data_events(meta):
    for line in meta.get("logMessages") or []:
        if line.startswith("Program data: "):
            yield base64.b64decode(line[len("Program data: ") :])


def trade_impact(ev):
    """Return single-trade price impact in bps, or None."""
    is_buy = ev[:8] == BUY_DISC
    if not is_buy and ev[:8] != SELL_DISC:
        return None
    o = 8 + 8  # disc + timestamp
    base_amount = struct.unpack_from("<Q", ev, o)[0]
    o += 8 * 4  # base_amount, limit, user_base, user_quote
    pool_base = struct.unpack_from("<Q", ev, o)[0]
    o += 8
    pool_quote = struct.unpack_from("<Q", ev, o)[0]
    o += 8
    quote_amount = struct.unpack_from("<Q", ev, o)[0]
    if pool_base == 0 or pool_quote == 0 or base_amount == 0:
        return None
    # event reserves are POST-trade; derive PRE-trade.
    if is_buy:
        pre_base, pre_quote = pool_base + base_amount, pool_quote - quote_amount
    else:
        pre_base, pre_quote = pool_base - base_amount, pool_quote + quote_amount
    if pre_base == 0 or pre_quote == 0:
        return None
    # price = quote per base; impact relative to pre.
    pre = pre_quote / pre_base
    post = pool_quote / pool_base
    return abs(post - pre) / pre * 10_000 if pre > 0 else None


def main():
    ps = pools(1000)
    solusdc = [p for p in ps if {p[1], p[2]} == {SOL, USDC}]
    print(f"pump_amm pools sampled: {len(ps)}")
    print(f"SOL/USDC markets found: {len(solusdc)}")
    print(
        "mint-class counts (baseSOL,quoteSOL,baseUSDC,quoteUSDC):",
        dict(collections.Counter((b == SOL, q == SOL, b == USDC, q == USDC) for _, b, q in ps)),
    )

    impacts = []
    for pool, _, _ in ps[:40]:
        sigs = rpc("getSignaturesForAddress", [pool, {"limit": 5}])
        if not sigs:
            continue
        for s in sigs["result"]:
            t = rpc(
                "getTransaction",
                [s["signature"], {"encoding": "json", "maxSupportedTransactionVersion": 1}],
            )
            if not t or not t.get("result"):
                continue
            for ev in program_data_events(t["result"]["meta"]):
                imp = trade_impact(ev)
                if imp is not None:
                    impacts.append(imp)
            if len(impacts) >= 200:
                break
        if len(impacts) >= 200:
            break
    if impacts:
        impacts.sort()
        print(f"single-trade price impact bps over {len(impacts)} trades:")
        print(f"  median={statistics.median(impacts):.1f}")
        print(f"  p90={impacts[int(len(impacts)*0.9)]:.1f}")
        print(f"  max={max(impacts):.1f}")
        print(f"  share > 100 bps (1%): {sum(1 for i in impacts if i > 100)/len(impacts):.1%}")
    else:
        print("no trades decoded")


if __name__ == "__main__":
    main()
