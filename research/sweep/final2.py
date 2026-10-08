import sys, json, base64, struct
sys.path.insert(0,"research/sweep")
from rpc import *
from concurrent.futures import ThreadPoolExecutor
RECIP="4UQeTP1T39KZ9Sfxzo3WR5skgsaP6NZa87BAkuazLEKH"
SOL_MINT="11111111111111111111111111111111"

class R:
    def __init__(s,b): s.b=b; s.o=0
    def u8(s):
        v=s.b[s.o]; s.o+=1; return v
    def u32(s):
        v=struct.unpack_from("<I",s.b,s.o)[0]; s.o+=4; return v
    def u64(s):
        v=struct.unpack_from("<Q",s.b,s.o)[0]; s.o+=8; return v
    def i64(s):
        v=struct.unpack_from("<q",s.b,s.o)[0]; s.o+=8; return v
    def pk(s):
        v=s.b[s.o:s.o+32]; s.o+=32; return b58e(v)
    def string(s):
        n=s.u32(); v=s.b[s.o:s.o+n].decode('utf8','replace'); s.o+=n; return v

def decode_trade_full(ev):
    r=R(ev); r.o=8
    d={}
    d["mint"]=r.pk(); d["sol_amount"]=r.u64(); d["token_amount"]=r.u64(); d["is_buy"]=r.u8()
    r.pk(); d["timestamp"]=r.i64()
    d["virtual_sol_reserves"]=r.u64(); d["virtual_token_reserves"]=r.u64()
    d["real_sol_reserves"]=r.u64(); d["real_token_reserves"]=r.u64()
    try:
        r.pk(); r.u64(); r.u64(); r.pk(); r.u64(); r.u64(); r.u8()
        r.u64(); r.u64(); r.u64(); r.i64()
        r.string(); r.u8(); r.u64(); r.u64(); r.u64(); r.u64()
        n=r.u32(); r.o+=n*(32+2)
        d["quote_mint"]=r.pk(); d["quote_amount"]=r.u64()
        d["virtual_quote_reserves"]=r.u64(); d["real_quote_reserves"]=r.u64()
    except Exception as e:
        d["tail_error"]=str(e)
    d.setdefault("quote_mint",SOL_MINT)
    vqr = d.get("virtual_quote_reserves")
    is_sol = d["sol_amount"]!=0 or d.get("quote_mint")==SOL_MINT
    d["quote_reserve_effective"] = d["virtual_sol_reserves"] if is_sol else vqr
    d["is_sol_pair"]=is_sol
    return d

def page(before=None, limit=1000):
    p=[RECIP,{"limit":limit}]
    if before: p[1]["before"]=before
    return [x["signature"] for x in rpc("getSignaturesForAddress",p)["result"]]

def collect(total=3000):
    sigs=[]; before=None
    while len(sigs)<total:
        b=page(before,min(1000,total-len(sigs)))
        if not b: break
        sigs+=b; before=b[-1]
    def scan(s):
        t=get_tx(s)
        if not t: return None
        sws=[decode_sweep(ev) for ev in program_data_events(t["meta"]) if ev[:8]==SWEEP_EVENT_DISC]
        return (s,t["slot"],sws) if sws else None
    out=[]
    with ThreadPoolExecutor(max_workers=12) as ex:
        for r in ex.map(scan,sigs):
            if r: out.append(r)
    return len(sigs), out

def account(curve):
    r=rpc("getAccountInfo",[curve,{"encoding":"base64"}]); v=r["result"]["value"]
    if not v: return None
    d=base64.b64decode(v["data"][0]); u=lambda o: struct.unpack_from("<Q",d,o)[0] if o+8<=len(d) else None
    return dict(len=len(d),vquote=u(16),creator_fee=u(125),protocol_fees=u(133),
                quote_mint=b58e(d[83:115]) if len(d)>=115 else None)

def timeline(curve,limit=40):
    sigs=[x["signature"] for x in rpc("getSignaturesForAddress",[curve,{"limit":limit}])["result"]]
    out=[]
    for s in sigs:
        t=get_tx(s)
        if not t: continue
        for ev in program_data_events(t["meta"]):
            if ev[:8]==TRADE_DISC:
                d=decode_trade_full(ev); d.update(slot=t["slot"],kind="trade",sig=s); out.append(d)
            elif ev[:8]==SWEEP_EVENT_DISC:
                d=decode_sweep(ev); d.update(slot=t["slot"],kind="sweep",sig=s); out.append(d)
    out.sort(key=lambda x:x["slot"])
    return out

def main():
    n,out=collect(3000)
    print("scanned sigs",n,"sweep txs",len(out))
    curves={}
    for s,slot,sws in out:
        for sw in sws: curves.setdefault(sw["curve"],[]).append(dict(sig=s,slot=slot,flag=sw["flag"],amount=sw["amount"]))
    keys=list(curves.keys())
    with ThreadPoolExecutor(max_workers=12) as ex:
        res=list(ex.map(lambda c:(c,timeline(c),account(c)), keys))
    rows=[]; skip=0
    for c,tl,acc in res:
        if not tl or acc is None: skip+=1; continue
        vq=acc["vquote"]
        # candidate last-write trades: those whose post quote reserve == account value
        matches=[e for e in tl if e["kind"]=="trade" and e["quote_reserve_effective"]==vq]
        if not matches:
            rows.append(dict(curve=c,status="no_matching_trade",acc_vq=vq,
                             trades=[(e["slot"],e["quote_reserve_effective"]) for e in tl if e["kind"]=="trade"][-4:])); continue
        last=max(matches,key=lambda e:e["slot"])
        sws=[s for s in curves[c] if s["slot"]>last["slot"]]
        if not sws: continue
        tau=sum(s["amount"] for s in sws)
        rows.append(dict(curve=c,status="ok",is_sol_pair=last.get("is_sol_pair"),
            quote_mint=acc["quote_mint"], last_trade_slot=last["slot"],
            sweep_slots=sorted(set(s["slot"] for s in sws)),
            instrs=sorted(set("SweepProtocolFee" if s["flag"]==0 else "SweepCreatorFee" for s in sws)),
            amounts=[s["amount"] for s in sws], swept_total=tau,
            pre_vq=last["quote_reserve_effective"], actual_post_vq=vq,

            zero=(last["quote_reserve_effective"]==vq),
            amount_hyp=(vq==last["quote_reserve_effective"]-tau),
            creator_fee=acc["creator_fee"], protocol_fees=acc["protocol_fees"],
            sigs=sorted(set(s["sig"] for s in sws))))
    ok=[r for r in rows if r.get("status")=="ok"]
    bad=[r for r in rows if r.get("status")!="ok"]
    print(f"curve rows ok={len(ok)} skipped={len(bad)}")
    print(f"  zero-change (post==pre): {sum(1 for r in ok if r['zero'])}")
    print(f"  amount-applied (post==pre-total): {sum(1 for r in ok if r['amount_hyp'])}")
    print(f"  SOL pairs: {sum(1 for r in ok if r['is_sol_pair'])}; non-SOL: {sum(1 for r in ok if not r['is_sol_pair'])}")
    for r in bad[:8]: print("  SKIP:",r)
    json.dump(ok, open("research/sweep/rows.json","w"), indent=1)
    for r in ok[:8]:
        print(f"  {r['curve'][:10]}.. instrs={r['instrs']} amt={r['amounts']} pre={r['pre_vq']} post={r['actual_post_vq']} zero={r['zero']} amount_hyp={r['amount_hyp']}")

if __name__=="__main__": main()
