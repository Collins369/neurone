#!/usr/bin/env python3
"""M2.2A: resolve pump.swap reserve semantics + SELL/BUY parity.

Uses per-transaction pre/postTokenBalances as ground truth for pool reserves,
restricts to single-event transactions (no multi-hop noise), and scores
candidate formulas by exact integer parity.
"""
import base64
import collections
import concurrent.futures
import json
import os
import sys
import time
import urllib.request

KEY = os.environ.get("SOLAMI_API_KEY", "")
if not KEY:
    for line in open(os.path.join(os.path.dirname(__file__), "..", ".env")):
        if line.startswith("SOLAMI_API_KEY="):
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
            return json.load(urllib.request.urlopen(req, timeout=30))
        except Exception:
            if k == tries - 1:
                return None
            time.sleep(0.7 * (k + 1))


def u16(b, o): return int.from_bytes(b[o:o + 2], "little")
def u64(b, o): return int.from_bytes(b[o:o + 8], "little")
def i64(b, o): return int.from_bytes(b[o:o + 8], "little", signed=True)
def i128(b, o): return int.from_bytes(b[o:o + 16], "little", signed=True)
def pkf(b, o): return b58(b[o:o + 32])


def decode_pool(data):
    """Decode the common prefix + version-specific tail of a Pool account."""
    o = 8
    bump = data[o]; o += 1
    index = u16(data, o); o += 2
    creator = pkf(data, o); o += 32
    base_mint = pkf(data, o); o += 32
    quote_mint = pkf(data, o); o += 32
    o += 32  # lp_mint
    base_ta = pkf(data, o); o += 32
    quote_ta = pkf(data, o); o += 32
    lp_supply = u64(data, o); o += 8
    o += 32  # coin_creator
    out = dict(len=len(data), bump=bump, index=index, creator=creator, base_mint=base_mint,
               quote_mint=quote_mint, base_ta=base_ta, quote_ta=quote_ta, lp_supply=lp_supply)
    if len(data) >= 244:
        out["is_mayhem"] = data[o]; o += 1
        out["is_cashback"] = data[o]; o += 1
    if len(data) >= 261:
        out["virtual_quote_reserves"] = i128(data, o); o += 16
    if len(data) >= 269:
        out["creator_fee_bps"] = u64(data, o); o += 8
    return out


def decode_swap(raw, is_buy):
    o = 8
    ts = i64(raw, o); o += 8
    base_amount = u64(raw, o); o += 8
    limit = u64(raw, o); o += 8
    ubase = u64(raw, o); o += 8
    uquote = u64(raw, o); o += 8
    pbase = u64(raw, o); o += 8
    pquote = u64(raw, o); o += 8
    quote_amount = u64(raw, o); o += 8
    lp_bps = u64(raw, o); o += 8
    lp_fee = u64(raw, o); o += 8
    proto_bps = u64(raw, o); o += 8
    proto_fee = u64(raw, o); o += 8
    q_with = u64(raw, o); o += 8
    uq_amount = u64(raw, o); o += 8
    pool = pkf(raw, o)
    return dict(is_buy=is_buy, ts=ts, base_amount=base_amount, limit=limit,
                ubase=ubase, uquote=uquote, pbase=pbase, pquote=pquote,
                quote_amount=quote_amount, lp_bps=lp_bps, lp_fee=lp_fee,
                proto_bps=proto_bps, proto_fee=proto_fee, q_with=q_with,
                uq_amount=uq_amount, pool=pool)


def collect(n_pools, sigs_per_pool, out_path):
    accs = rpc("getProgramAccountsV2", [PAMM, {
        "encoding": "base64", "filters": [{"memcmp": {"offset": 0, "bytes": b58(POOL)}}],
        "limit": 400}])["result"]["value"]["accounts"]
    pools = {}
    for a in accs:
        d = base64.b64decode(a["account"]["data"][0])
        if len(d) < 243:
            continue
        try:
            pools[a["pubkey"]] = decode_pool(d)
        except Exception:
            pass
    print(f"pools decoded: {len(pools)} sizes "
          f"{collections.Counter(p['len'] for p in pools.values()).most_common()}", file=sys.stderr)

    tasks = []
    keys = list(pools)[:n_pools]
    with concurrent.futures.ThreadPoolExecutor(max_workers=10) as ex:
        def sigs(pk):
            r = rpc("getSignaturesForAddress", [pk, {"limit": sigs_per_pool}])
            return (pk, [s["signature"] for s in (r.get("result") or [])]) if r else (pk, [])
        for pk, ss in ex.map(sigs, keys):
            for s in ss:
                tasks.append((pk, s))
    print(f"tx candidates: {len(tasks)}", file=sys.stderr)

    rows = []

    def fetch(task):
        pk, sig = task
        t = rpc("getTransaction", [sig, {"encoding": "jsonParsed", "maxSupportedTransactionVersion": 0}])
        if not t or not t.get("result"):
            return []
        res = t["result"]; meta = res["meta"]; msg = res["transaction"]["message"]
        keys_ = [k["pubkey"] if isinstance(k, dict) else k for k in msg["accountKeys"]]
        p = pools[pk]
        bi = keys_.index(p["base_ta"]) if p["base_ta"] in keys_ else None
        qi = keys_.index(p["quote_ta"]) if p["quote_ta"] in keys_ else None
        pre = {b["accountIndex"]: b for b in (meta.get("preTokenBalances") or [])}
        post = {b["accountIndex"]: b for b in (meta.get("postTokenBalances") or [])}
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
                evs.append(decode_swap(raw, True))
            elif raw[:8] == SELL:
                evs.append(decode_swap(raw, False))
        if len(evs) != 1:
            return []  # single-event transactions only
        e = evs[0]
        e["pool"] = pk
        e["slot"] = res["slot"]
        e["sig"] = sig
        e["pool_len"] = p["len"]
        e["virtual_quote_reserves"] = p.get("virtual_quote_reserves", 0)
        e["bal_base_pre"] = bal(pre, bi)
        e["bal_base_post"] = bal(post, bi)
        e["bal_quote_pre"] = bal(pre, qi)
        e["bal_quote_post"] = bal(post, qi)
        return [e]

    with concurrent.futures.ThreadPoolExecutor(max_workers=12) as ex:
        for i, out in enumerate(ex.map(fetch, tasks)):
            rows.extend(out)
            # Flush periodically so a timeout still leaves usable data.
            if i % 250 == 0:
                json.dump(rows, open(out_path, "w"))
                print(f"  ...{i}/{len(tasks)} txs, {len(rows)} swaps", file=sys.stderr)
    json.dump(rows, open(out_path, "w"))
    print(f"saved {len(rows)} single-event swaps -> {out_path}")


def analyze(path):
    rows = json.load(open(path))
    print(f"\n=== pump.swap single-event swaps: {len(rows)} ===")
    print("pool sizes:", collections.Counter(r["pool_len"] for r in rows).most_common())
    # semantics: event reserves vs balance pre
    sem = collections.Counter()
    for r in rows:
        if r["bal_base_pre"] is None:
            continue
        sem["n"] += 1
        if int(r["bal_base_pre"]) == r["pbase"] and int(r["bal_quote_pre"]) == r["pquote"]:
            sem["event==pre"] += 1
        if int(r["bal_base_post"]) == r["pbase"] and int(r["bal_quote_post"]) == r["pquote"]:
            sem["event==post"] += 1
    print("reserve semantics vs token balances:", dict(sem))
    # fee regimes
    print("fee regimes (lp_bps,proto_bps):",
          collections.Counter((r["lp_bps"], r["proto_bps"]) for r in rows).most_common(6))
    # SELL parity
    for name, fn in {
        "gross_cp": lambda r: r["pquote"] * r["base_amount"] // (r["pbase"] + r["base_amount"]),
        "gross_cp_virt": lambda r: (r["pquote"] + r["virtual_quote_reserves"]) * r["base_amount"]
        // (r["pbase"] + r["base_amount"]),
        "gross_cp_minus1": lambda r: r["pquote"] * (r["base_amount"] - 1) // (r["pbase"] + r["base_amount"] - 1),
    }.items():
        ok = t = 0
        maxrel = 0.0
        for r in rows:
            if r["is_buy"]:
                continue
            t += 1
            v = fn(r)
            if v == r["quote_amount"]:
                ok += 1
            else:
                maxrel = max(maxrel, abs(v - r["quote_amount"]) / max(r["quote_amount"], 1))
        print(f"SELL {name:16} exact={ok}/{t} maxrel={maxrel:.3e}")
    # BUY parity
    for name, fn in {
        "gross_in": lambda r: r["pbase"] * r["quote_amount"] // (r["pquote"] + r["quote_amount"]),
        "q_with": lambda r: r["pbase"] * r["q_with"] // (r["pquote"] + r["q_with"]),
        "uq_amount": lambda r: r["pbase"] * r["uq_amount"] // (r["pquote"] + r["uq_amount"]),
        "virt_q_with": lambda r: r["pbase"] * r["q_with"] // (r["pquote"] + r["virtual_quote_reserves"] + r["q_with"]),
    }.items():
        ok = t = 0
        maxrel = 0.0
        for r in rows:
            if not r["is_buy"]:
                continue
            t += 1
            v = fn(r)
            if v == r["base_amount"]:
                ok += 1
            else:
                maxrel = max(maxrel, abs(v - r["base_amount"]) / max(r["base_amount"], 1))
        print(f"BUY  {name:16} exact={ok}/{t} maxrel={maxrel:.3e}")


if __name__ == "__main__":
    cmd = sys.argv[1] if len(sys.argv) > 1 else "analyze"
    if cmd == "collect":
        collect(int(sys.argv[2]), int(sys.argv[3]), sys.argv[4])
    else:
        analyze(sys.argv[2] if len(sys.argv) > 2 else "/tmp/ps_single.json")
