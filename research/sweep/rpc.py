import os, re, json, urllib.request, base64, struct
URL=f"https://rpc.solami.dev/sol?api_key={os.environ['SOLAMI_RPC_API_KEY']}"
def rpc(m,p,tries=5):
    last=None
    for _ in range(tries):
        try:
            b=json.dumps({"jsonrpc":"2.0","id":1,"method":m,"params":p}).encode()
            r=json.load(urllib.request.urlopen(urllib.request.Request(URL,data=b,headers={"Content-Type":"application/json"}),timeout=90))
            if "error" in r: last=r; continue
            return r
        except Exception as e:
            last=e
    raise SystemExit(f"rpc {m} failed: {last}")

A="123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
def b58d(s):
    n=0
    for c in s: n=n*58+A.index(c)
    raw=b"" if n==0 else n.to_bytes((n.bit_length()+7)//8,"big")
    return b"\x00"*(len(s)-len(s.lstrip('1')))+raw
def b58e(b):
    n=int.from_bytes(b,"big"); out=""
    while n>0: n,r=divmod(n,58); out=A[r]+out
    return "1"*(len(b)-len(b.lstrip(b"\x00")))+(out or "")

PUMP="6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"
TRADE_DISC=bytes.fromhex("bddb7fd34ee661ee")
SWEEP_EVENT_DISC=bytes.fromhex("742b4dbd117a482b")
SWEEP_PROTOCOL=bytes.fromhex("0830be07b644b7e5")
SWEEP_CREATOR=bytes.fromhex("20f6bf3408c949ba")

def get_tx(sig, ver=1):
    for _ in range(4):
        r=rpc("getTransaction",[sig,{"encoding":"json","maxSupportedTransactionVersion":ver}],tries=3)
        err=r.get("error")
        if err:
            m=re.search(r"version \((\d+)\)", err.get("message",""))
            if m: ver=int(m.group(1)); continue
        return r.get("result")
    return None

def program_data_events(meta):
    out=[]
    for l in meta.get("logMessages") or []:
        if l.startswith("Program data: "):
            out.append(base64.b64decode(l[len("Program data: "):]))
    return out

def decode_trade(ev):
    # disc(8) mint(32) sol(u64) tok(u64) is_buy(bool) user(32) ts(i64)
    o=8
    mint=b58e(ev[o:o+32]); o+=32
    sol=struct.unpack_from("<Q",ev,o)[0]; o+=8
    tok=struct.unpack_from("<Q",ev,o)[0]; o+=8
    is_buy=ev[o]!=0; o+=1
    o+=32
    ts=struct.unpack_from("<q",ev,o)[0]; o+=8
    vsol=struct.unpack_from("<Q",ev,o)[0]; o+=8
    vtok=struct.unpack_from("<Q",ev,o)[0]; o+=8
    rsol=struct.unpack_from("<Q",ev,o)[0]; o+=8
    rtok=struct.unpack_from("<Q",ev,o)[0]; o+=8
    return dict(mint=mint,sol=sol,tok=tok,is_buy=is_buy,ts=ts,vsol=vsol,vtok=vtok,rsol=rsol,rtok=rtok)

def decode_sweep(ev):
    o=8
    ts=struct.unpack_from("<Q",ev,o)[0]; o+=8
    mint=b58e(ev[o:o+32]); o+=32
    curve=b58e(ev[o:o+32]); o+=32
    p1=b58e(ev[o:o+32]); o+=32
    p2=b58e(ev[o:o+32]); o+=32
    amount=struct.unpack_from("<Q",ev,o)[0]; o+=8
    flag=ev[o]; o+=1
    return dict(ts=ts,mint=mint,curve=curve,p1=p1,p2=p2,amount=amount,flag=flag)
