#!/usr/bin/env python3
"""M2.2B: pump.swap parity using the event's own appended virtual_quote_reserves.

Official docs state BuyEvent/SellEvent include `virtual_quote_reserves`
(appended), so effective quote reserves are reconstructible per event.

  effective_quote = pool_quote_token_reserves + virtual_quote_reserves (signed i128)
  pricing: buy  = pool.buy(baseOut, maxQuoteIn)   -> quote derived from baseOut
           sell = pool.sell(baseIn, minQuoteOut)  -> quote derived from baseIn
"""
import base64
import collections
import concurrent.futures
import json
import os
import sys
import time
import urllib.request

KEY = os.environ.get("SOLAMI_RPC_API_KEY") or os.environ.get("SOLAMI_API_KEY", "")
if not KEY:
    for line in open(os.path.join(os.path.dirname(__file__), "..", ".env")):
        if line.startswith(("SOLAMI_RPC_API_KEY=", "SOLAMI_API_KEY=")):
            KEY = line.split("=", 1)[1].strip()
URL = f"https://rpc.solami.dev/sol?api_key={KEY}"
PAMM = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"
A = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
POOL = bytes([241, 154, 109, 4, 17, 177, 109, 188])
BUY = bytes([103, 244, 82, 31, 44, 245, 119, 119])
SELL = bytes([62, 47, 55, 10, 165, 3, 220, 42])


def b58(b):
    n = int.from_bytes(b, "big"); e = ""
    while n > 0:
        n, r = divmod(n, 58); e = A[r] + e
    return "1" * (len(b) - len(b.lstrip(b"\x00"))) + e


def rpc(method, params, tries=4):
    for k in range(tries):
        try:
            body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
            req = urllib.request.Request(URL, data=body, headers={"Content-Type": "application/json"})
            return json.load(urllib.request.urlopen(req, timeout=25))
        except Exception:
            if k == tries - 1:
                return None
            time.sleep(0.6 * (k + 1))


def u16(b, o): return int.from_bytes(b[o:o + 2], "little")
def u64(b, o): return int.from_bytes(b[o:o + 8], "little")
def i64(b, o): return int.from_bytes(b[o:o + 8], "little", signed=True)
def i128(b, o): return int.from_bytes(b[o:o + 16], "little", signed=True)
def pkf(b, o): return b58(b[o:o + 32])
def rust_string(b, o):
    n = int.from_bytes(b[o:o + 4], "little")
    return b[o + 4:o + 4 + n].decode("utf-8", "replace"), o + 4 + n


def decode_pool(data):
    o = 8 + 1 + 2 + 32 * 4  # disc, bump, index, creator, base, quote, lp_mint
    base_ta = pkf(data, o); o += 32
    quote_ta = pkf(data, o); o += 32
    out = dict(len=len(data), base_ta=base_ta, quote_ta=quote_ta, virt_account=0)
    if len(data) >= 261:
        # bump(1) index(2) creator(32) base(32) quote(32) lp(32) base_ta(32) quote_ta(32) lp_supply(8) coin_creator(32)
        out["virt_account"] = i128(data, 245)
    return out


def decode_event(raw, is_buy):
    o = 8
    ts = i64(raw, o); o += 8
    base_amount = u64(raw, o); o += 8
    limit = u64(raw, o); o += 8
    o += 8  # user_base_reserves
    o += 8  # user_quote_reserves
    pbase = u64(raw, o); o += 8
    pquote = u64(raw, o); o += 8
    quote_amount = u64(raw, o); o += 8
    lp_bps = u64(raw, o); o += 8
    lp_fee = u64(raw, o); o += 8
    proto_bps = u64(raw, o); o += 8
    proto_fee = u64(raw, o); o += 8
    q_with = u64(raw, o); o += 8
    uq_amount = u64(raw, o); o += 8
    o += 32   # pool
    o += 32 * 5  # user, user_base_ta, user_quote_ta, fee_recipient, fee_recipient_ta
    o += 32   # coin_creator
    coin_creator_fee_bps = u64(raw, o); o += 8
    coin_creator_fee = u64(raw, o); o += 8
    ix = ""
    if is_buy:
        # track_volume(1) + 5 u64 (totals/current_sol_volume/last_update_ts + 1 more)
        o += 1 + 8 * 5
        if len(raw) > o:
            ix, o = rust_string(raw, o)
        o += 8 + 8 + 8 + 8  # cashback_bps, cashback, buyback_bps, buyback_fee
    else:
        o += 8 + 8 + 8 + 8  # cashback_bps, cashback, buyback_bps, buyback_fee
    virt = i128(raw, o) if len(raw) >= o + 16 else 0
    return dict(is_buy=is_buy, ts=ts, base_amount=base_amount, limit=limit,
                pbase=pbase, pquote=pquote, quote_amount=quote_amount,
                lp_bps=lp_bps, lp_fee=lp_fee, proto_bps=proto_bps, proto_fee=proto_fee,
                q_with=q_with, uq_amount=uq_amount, coin_creator_fee=coin_creator_fee,
                coin_creator_fee_bps=coin_creator_fee_bps, ix_name=ix, virt_event=virt)


def collect(n_pools, sigs_per_pool, out_path):
    accs = rpc("getProgramAccountsV2", [PAMM, {
        "encoding": "base64", "filters": [{"memcmp": {"offset": 0, "bytes": b58(POOL)}}],
        "limit": 400}])["result"]["value"]["accounts"]
    pools = {}
    for a in accs:
        d = base64.b64decode(a["account"]["data"][0])
        if len(d) >= 243:
            pools[a["pubkey"]] = decode_pool(d)
    print(f"pools: {len(pools)}", file=sys.stderr)
    tasks = []

    def sigs(pk):
        r = rpc("getSignaturesForAddress", [pk, {"limit": sigs_per_pool}])
        return [(pk, s["signature"]) for s in (r.get("result") or [])] if r else []

    with concurrent.futures.ThreadPoolExecutor(max_workers=12) as ex:
        for out in ex.map(sigs, list(pools)[:n_pools]):
            tasks.extend(out)
    print(f"txs: {len(tasks)}", file=sys.stderr)
    rows = []

    def fetch(task):
        pk, sig = task
        t = rpc("getTransaction", [sig, {"encoding": "jsonParsed", "maxSupportedTransactionVersion": 0}])
        if not t or not t.get("result"):
            return []
        res = t["result"]; meta = res["meta"]; msg = res["transaction"]["message"]
        keys = [k["pubkey"] if isinstance(k, dict) else k for k in msg["accountKeys"]]
        p = pools[pk]
        bi = keys.index(p["base_ta"]) if p["base_ta"] in keys else None
        qi = keys.index(p["quote_ta"]) if p["quote_ta"] in keys else None
        pre = {b["accountIndex"]: b for b in (meta.get("preTokenBalances") or [])}
        bal = lambda d, i: (d.get(i, {}).get("uiTokenAmount", {}) or {}).get("amount")
        evs = []
        for line in (meta.get("logMessages") or []):
            if not line.startswith("Program data:"):
                continue
            try:
                raw = base64.b64decode(line.split(":", 1)[1].strip())
            except Exception:
                continue
            if raw[:8] == BUY:
                evs.append(decode_event(raw, True))
            elif raw[:8] == SELL:
                evs.append(decode_event(raw, False))
        if len(evs) != 1:
            return []
        e = evs[0]
        e["pool"] = pk
        e["pool_len"] = p["len"]
        e["virt_account"] = p["virt_account"]
        e["bal_base_pre"] = bal(pre, bi)
        e["bal_quote_pre"] = bal(pre, qi)
        return [e]

    with concurrent.futures.ThreadPoolExecutor(max_workers=12) as ex:
        for i, out in enumerate(ex.map(fetch, tasks)):
            rows.extend(out)
            if i % 250 == 0:
                json.dump(rows, open(out_path, "w"))
    json.dump(rows, open(out_path, "w"))
    print(f"saved {len(rows)} -> {out_path}")


def cd(a, b): return -(-a // b)


def analyze(path):
    rows = json.load(open(path))
    print(f"\n=== pump.swap single-event swaps: {len(rows)} ===")
    print("regimes:", collections.Counter((r["lp_bps"], r["proto_bps"]) for r in rows).most_common(8))
    print("virt_event==virt_account:",
          sum(1 for r in rows if r["virt_event"] == r["virt_account"]), "/", len(rows))
    print("virt_event nonzero:", sum(1 for r in rows if r["virt_event"] != 0))
    for side in (False, True):
        rs = [r for r in rows if r["is_buy"] == side]
        if not rs:
            continue
        # SELL: quote_out from base_in.  BUY: base_out given, quote derived.
        for label, extra in (("event_virt", 0), ("acct_virt", 1), ("no_virt", 2)):
            ok = tot = 0
            for r in rs:
                v = r["virt_event"] if extra == 0 else (r["virt_account"] if extra == 1 else 0)
                qeff = r["pquote"] + v
                if side:  # buy: specified base_amount_out -> derived quote
                    if r["pbase"] <= r["base_amount"]:
                        continue
                    pred = cd(qeff * r["base_amount"], r["pbase"] - r["base_amount"])
                    obs = r["quote_amount"]
                else:     # sell: specified base_amount_in -> quote out
                    pred = qeff * r["base_amount"] // (r["pbase"] + r["base_amount"])
                    obs = r["quote_amount"]
                tot += 1
                ok += pred == obs
            print(f"  {'BUY ' if side else 'SELL'} {label:10} exact={ok}/{tot}")


if __name__ == "__main__":
    if sys.argv[1] == "collect":
        collect(int(sys.argv[2]), int(sys.argv[3]), sys.argv[4])
    else:
        analyze(sys.argv[2] if len(sys.argv) > 2 else "/tmp/ps_ev.json")
