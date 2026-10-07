#!/usr/bin/env python3
"""Differential BUY-quote parity analysis over collected mainnet samples.

Reads /tmp/pf_samples.json and /tmp/ps_samples.json (from buy_parity.py collect)
and reports reserve pre/post semantics + candidate formula exact-match counts,
partitioned by instruction name.
"""
import collections
import json
import os


def ceil_div(a, b):
    return -(-a // b)


def semantics(events, venue):
    """Return (pre_is_own, post_is_own) match counts over consecutive pairs."""
    by_pool = collections.defaultdict(list)
    for e in events:
        by_pool[e.get("pool_key") or e.get("mint")].append(e)
    pre = post = 0
    for _, evs in by_pool.items():
        evs.sort(key=lambda e: e["slot"])
        for a, b in zip(evs, evs[1:]):
            if venue == "pumpfun":
                db, dq = b["vtok"] - a["vtok"], b["vsol"] - a["vsol"]
                # effect of a: buy -> +sol, -tok ; sell -> -sol, +tok
                if a["is_buy"]:
                    post += (dq == a["sol"] and -db == a["tok"])
                    pre += (dq == b["sol"] and -db == b["tok"])
                else:
                    post += (dq == -a["sol"] and -db == -a["tok"])
                    pre += (dq == -b["sol"] and -db == -b["tok"])
            else:
                db, dq = b["pbase"] - a["pbase"], b["pquote"] - a["pquote"]
                if a["is_buy"]:
                    post += (-db == a["base_amount"] and dq == a["quote_amount"])
                    pre += (-db == b["base_amount"] and dq == b["quote_amount"])
                else:
                    post += (db == a["base_amount"] and -dq == a["quote_amount"])
                    pre += (db == b["base_amount"] and -dq == b["quote_amount"])
    return pre, post


def analyze_pumpfun(path):
    s = json.load(open(path))
    print(f"\n===== pump.fun: {len(s)} events =====")
    sec, first = semantics(s, "pumpfun")
    # "second-event-delta" == POST-trade reserves; "first-event-delta" == PRE.
    print(f"reserve semantics: post_trade={sec} pre_trade={first}")
    by_pool = collections.defaultdict(list)
    for e in s:
        by_pool[e["mint"]].append(e)

    # SELL parity with PRE = predecessor's reserves.
    res = collections.Counter()
    for evs in by_pool.values():
        evs.sort(key=lambda e: e["slot"])
        for a, b in zip(evs, evs[1:]):
            if b["is_buy"]:
                continue
            gross = a["vsol"] * b["tok"] // (a["vtok"] + b["tok"])
            res["sell_total"] += 1
            res["sell_exact"] += gross == b["sol"]
    print(f"SELL gross = vq*tok/(vb+tok): {res['sell_exact']}/{res['sell_total']}")

    # BUY candidates, partitioned by ix_name.
    cand = collections.Counter()
    fail_examples = collections.defaultdict(list)
    for evs in by_pool.values():
        evs.sort(key=lambda e: e["slot"])
        for a, b in zip(evs, evs[1:]):
            if not b["is_buy"]:
                continue
            vsol, vtok = a["vsol"], a["vtok"]
            sol, tok = b["sol"], b["tok"]
            if sol == 0 or tok == 0 or vsol == 0 or vtok == 0:
                continue
            if b.get("ix_name", "?").startswith("sell"):
                continue
            bps, cbps = b["fee_bps"], b["creator_bps"]
            fee = sol - sol * (10000 - bps) // 10000
            cfee = sol - sol * (10000 - cbps) // 10000
            ix = b.get("ix_name", "?")
            tests = {
                "gross_cp": vtok * sol // (vsol + sol),
                "net_protofee": vtok * (sol - fee) // (vsol + (sol - fee)),
                "net_total_fee": vtok * (sol - fee - cfee) // (vsol + (sol - fee - cfee)),
                "sol_minus_1": vtok * (sol - 1) // (vsol + sol - 1),
            }
            for name, val in tests.items():
                cand[f"{ix}|{name}_total"] += 1
                cand[f"{ix}|{name}_ok"] += val == tok
                if val != tok and len(fail_examples[f"{ix}|{name}"]) < 2:
                    fail_examples[f"{ix}|{name}"].append(
                        dict(sol=sol, tok=tok, pred=val, diff=val - tok,
                             vsol=vsol, vtok=vtok, bps=bps, cbps=cbps, fee=fee, cfee=cfee))
            # token-target inverse (native direction for `buy`)
            if vtok > tok:
                need = ceil_div(vsol * tok, vtok - tok)
                cand[f"{ix}|token_target_need==sol_total"] += 1
                cand[f"{ix}|token_target_need==sol_ok"] += need == sol
                cand[f"{ix}|token_target_need+fee==sol_ok"] += need + fee + cfee == sol
                if need != sol and len(fail_examples[f"{ix}|token_target"]) < 2:
                    fail_examples[f"{ix}|token_target"].append(
                        dict(sol=sol, tok=tok, need=need, sol_need=need - sol, fee=fee, cfee=cfee))
    for key in sorted(cand):
        if key.endswith("_total") or key.endswith("_ok"):
            print(f"  {key}: {cand[key]}")
    for k in sorted(fail_examples):
        print(f"  examples [{k}]:")
        for ex in fail_examples[k]:
            print("    ", ex)


def analyze_pumpswap(path):
    s = json.load(open(path))
    print(f"\n===== pump.swap: {len(s)} events =====")
    sec, first = semantics(s, "pumpswap")
    print(f"reserve semantics: post_trade={sec} pre_trade={first}")
    by_pool = collections.defaultdict(list)
    for e in s:
        by_pool[e["pool"]].append(e)
    res = collections.Counter()
    for evs in by_pool.values():
        evs.sort(key=lambda e: e["slot"])
        for a, b in zip(evs, evs[1:]):
            if b["is_buy"]:
                continue
            gross = a["pquote"] * b["base_amount"] // (a["pbase"] + b["base_amount"])
            res["sell_total"] += 1
            res["sell_exact"] += gross == b["quote_amount"]
    print(f"SELL gross (pre=predecessor) = q*base/(b+base): {res['sell_exact']}/{res['sell_total']}")
    # own-reserves-as-pre variant
    res2 = collections.Counter()
    for e in s:
        if e["is_buy"]:
            continue
        gross = e["pquote"] * e["base_amount"] // (e["pbase"] + e["base_amount"])
        res2["t"] += 1
        res2["ok"] += gross == e["quote_amount"]
    print(f"SELL gross (pre=own reserves): {res2['ok']}/{res2['t']}")
    # BUY candidates with pre = predecessor
    cand = collections.Counter()
    ex = collections.defaultdict(list)
    for evs in by_pool.values():
        evs.sort(key=lambda e: e["slot"])
        for a, b in zip(evs, evs[1:]):
            if not b["is_buy"]:
                continue
            pb, pq = a["pbase"], a["pquote"]
            gross_out = b["base_amount"]
            tests = {
                "gross_in": pb * b["quote_amount"] // (pq + b["quote_amount"]),
                "q_with": pb * b["q_with"] // (pq + b["q_with"]),
                "uq_amount": pb * b["uq_amount"] // (pq + b["uq_amount"]),
            }
            for name, val in tests.items():
                cand[f"{name}_t"] += 1
                cand[f"{name}_ok"] += val == gross_out
                if val != gross_out and len(ex[name]) < 2:
                    ex[name].append(dict(out=gross_out, pred=val, diff=val - gross_out,
                                         quote_amount=b["quote_amount"], q_with=b["q_with"],
                                         uq=b["uq_amount"], pb=pb, pq=pq))
    for k in sorted(cand):
        print(f"  {k}: {cand[k]}")
    for k in ex:
        print(f"  examples [{k}]:")
        for e in ex[k]:
            print("    ", e)


if __name__ == "__main__":
    if os.path.exists("/tmp/pf_samples.json"):
        analyze_pumpfun("/tmp/pf_samples.json")
    if os.path.exists("/tmp/ps_samples.json"):
        analyze_pumpswap("/tmp/ps_samples.json")
