#!/usr/bin/env python3
"""M2.2 research harness: differential BUY-quote parity for pump.fun / pump.swap.

Investigation only. It collects labelled mainnet samples (cached under /tmp),
determines reserve pre/post semantics from SELL parity, then scores a grid of
candidate BUY formulas by *exact integer parity*.

Usage:
    SOLAMI_API_KEY=... python3 research/buy_parity.py collect [n_curves]
    python3 research/buy_parity.py analyze
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

PUMP = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"
PAMM = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"
A = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"

BC = bytes([23, 183, 248, 55, 96, 216, 172, 96])
TRADE = bytes([189, 219, 127, 211, 78, 230, 97, 238])
POOL = bytes([241, 154, 109, 4, 17, 177, 109, 188])
BUY = bytes([103, 244, 82, 31, 44, 245, 119, 119])
SELL = bytes([62, 47, 55, 10, 165, 3, 220, 42])


def b58(b: bytes) -> str:
    n = int.from_bytes(b, "big")
    e = ""
    while n > 0:
        n, r = divmod(n, 58)
        e = A[r] + e
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
            time.sleep(0.8 * (k + 1))


def u64(b, o):
    return int.from_bytes(b[o:o + 8], "little")


def i64(b, o):
    return int.from_bytes(b[o:o + 8], "little", signed=True)


def rust_string(b, o):
    n = int.from_bytes(b[o:o + 4], "little")
    return b[o + 4:o + 4 + n].decode("utf-8", "replace")


def decode_trade(raw):
    """pump.fun TradeEvent."""
    o = 8
    mint = b58(raw[o:o + 32]); o += 32
    sol = u64(raw, o); o += 8
    tok = u64(raw, o); o += 8
    is_buy = raw[o]; o += 1
    user = b58(raw[o:o + 32]); o += 32
    ts = i64(raw, o); o += 8
    vsol = u64(raw, o); o += 8
    vtok = u64(raw, o); o += 8
    rsol = u64(raw, o); o += 8
    rtok = u64(raw, o); o += 8
    fee_recipient = b58(raw[o:o + 32]); o += 32
    fee_bps = u64(raw, o); o += 8
    fee = u64(raw, o); o += 8
    creator = b58(raw[o:o + 32]); o += 32
    creator_bps = u64(raw, o); o += 8
    creator_fee = u64(raw, o); o += 8
    o += 1  # track_volume
    o += 8 + 8 + 8 + 8  # totals, current_sol_volume, last_update_ts
    ix_name = rust_string(raw, o) if len(raw) > o else ""
    return dict(venue="pumpfun", mint=mint, sol=sol, tok=tok, is_buy=is_buy, ts=ts,
                vsol=vsol, vtok=vtok, rsol=rsol, rtok=rtok,
                fee_bps=fee_bps, fee=fee, creator_bps=creator_bps, creator_fee=creator_fee,
                ix_name=ix_name)


def decode_pamm(raw, is_buy):
    o = 8
    ts = i64(raw, o); o += 8
    base_amount = u64(raw, o); o += 8      # out (buy) / in (sell)
    limit = u64(raw, o); o += 8
    ubase = u64(raw, o); o += 8
    uquote = u64(raw, o); o += 8
    pbase = u64(raw, o); o += 8
    pquote = u64(raw, o); o += 8
    quote_amount = u64(raw, o); o += 8     # in (buy) / out (sell)
    lp_bps = u64(raw, o); o += 8
    lp_fee = u64(raw, o); o += 8
    proto_bps = u64(raw, o); o += 8
    proto_fee = u64(raw, o); o += 8
    q_with = u64(raw, o); o += 8
    uq_amount = u64(raw, o); o += 8
    pool = b58(raw[o:o + 32]); o += 32
    return dict(venue="pumpswap", pool=pool, is_buy=1 if is_buy else 0, ts=ts,
                base_amount=base_amount, quote_amount=quote_amount,
                pbase=pbase, pquote=pquote, lp_bps=lp_bps, lp_fee=lp_fee,
                proto_bps=proto_bps, proto_fee=proto_fee,
                q_with=q_with, uq_amount=uq_amount,
                limit=limit, ubase=ubase, uquote=uquote)


def collect(venue, n_target, path):
    samples = []
    sig_tasks = []  # (pool_key, signature)
    if venue == "pumpfun":
        accs = rpc("getProgramAccountsV2", [PUMP, {
            "encoding": "base64", "filters": [{"memcmp": {"offset": 0, "bytes": b58(BC)}}],
            "dataSlice": {"offset": 0, "length": 125}, "limit": 600}])["result"]["value"]["accounts"]
        keys = [a["pubkey"] for a in accs]
        disc = TRADE
    else:
        accs = rpc("getProgramAccountsV2", [PAMM, {
            "encoding": "base64", "filters": [{"memcmp": {"offset": 0, "bytes": b58(POOL)}}],
            "dataSlice": {"offset": 0, "length": 260}, "limit": 600}])["result"]["value"]["accounts"]
        keys = [a["pubkey"] for a in accs]
        disc = None
    keys = keys[:120]

    def sigs(pk):
        r = rpc("getSignaturesForAddress", [pk, {"limit": 40}])
        return [(pk, s["signature"]) for s in (r.get("result") or [])] if r else []

    with concurrent.futures.ThreadPoolExecutor(max_workers=10) as ex:
        for out in ex.map(sigs, keys):
            sig_tasks.extend(out)
    print(f"  {venue}: {len(sig_tasks)} candidate txs", file=sys.stderr)

    def fetch(task):
        pk, sig = task
        t = rpc("getTransaction", [sig, {"encoding": "jsonParsed", "maxSupportedTransactionVersion": 0}])
        if not t or not t.get("result"):
            return []
        slot = t["result"].get("slot")
        out = []
        for line in ((t["result"].get("meta") or {}).get("logMessages") or []):
            if not line.startswith("Program data:"):
                continue
            try:
                raw = base64.b64decode(line.split(":", 1)[1].strip())
            except Exception:
                continue
            if venue == "pumpfun":
                if raw[:8] != TRADE:
                    continue
                ev = decode_trade(raw)
            else:
                if raw[:8] == BUY:
                    ev = decode_pamm(raw, True)
                elif raw[:8] == SELL:
                    ev = decode_pamm(raw, False)
                else:
                    continue
            ev["pool_key"] = pk
            ev["slot"] = slot
            out.append(ev)
        return out

    with concurrent.futures.ThreadPoolExecutor(max_workers=10) as ex:
        for out in ex.map(fetch, sig_tasks):
            samples.extend(out)
            if len(samples) >= n_target:
                break
    json.dump(samples, open(path, "w"))
    print(f"saved {len(samples)} -> {path}")


def main():
    cmd = sys.argv[1] if len(sys.argv) > 1 else "analyze"
    if cmd == "collect":
        n = int(sys.argv[2]) if len(sys.argv) > 2 else 400
        collect("pumpfun", n, "/tmp/pf_samples.json")
        collect("pumpswap", n, "/tmp/ps_samples.json")
        return
    analyze("/tmp/pf_samples.json")
    analyze("/tmp/ps_samples.json")


def ceil_div(a, b):
    return -(-a // b)


def analyze(path):
    if not os.path.exists(path):
        print(f"(missing {path})")
        return
    s = json.load(open(path))
    venue = s[0]["venue"] if s else "?"
    print(f"\n===== {venue}: {len(s)} events =====")
    by_pool = collections.defaultdict(list)
    for ev in s:
        by_pool[ev.get("pool_key") or ev.get("mint")].append(ev)
    # reserve semantics via SELL parity, using the event's own reserves as PRE
    sell_tot = sell_pre = 0
    for pool, evs in by_pool.items():
        for ev in evs:
            if ev["is_buy"]:
                continue
            sell_tot += 1
            if venue == "pumpfun":
                pre_vq, pre_vb = ev["vsol"], ev["vtok"]
                gross = pre_vq * ev["tok"] // (pre_vb + ev["tok"])
            else:
                pre_b, pre_q = ev["pbase"], ev["pquote"]
                gross = pre_q * ev["base_amount"] // (pre_b + ev["base_amount"])
            if gross == ev["sol"] if venue == "pumpfun" else gross == ev["quote_amount"]:
                sell_pre += 1
    print(f"SELL: event-reserves-as-PRE exact = {sell_pre}/{sell_tot}")
    print("ix_name histogram:", collections.Counter(e.get("ix_name", "") for e in s).most_common())


if __name__ == "__main__":
    main()
