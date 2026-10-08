#!/usr/bin/env python3
"""Evidence for PUMPSWAP_LATE_BOOTSTRAP_REPORT.md (read-only, no production code).

Shows that each PumpSwap pool's two vault token accounts are owned by the SPL
Token program (Tokenkeg/Tokenz) and their authority field is the *pool* itself,
i.e. it is pool-specific. Therefore no bounded, static Yellowstone account
filter can select "the vaults of all pump_amm pools".

Run: SOLAMI_RPC_API_KEY=... python3 research/pumpswap_bootstrap/probe.py
"""
import base64
import collections
import json
import os
import struct
import urllib.request

URL = f"https://rpc.solami.dev/sol?api_key={os.environ['SOLAMI_RPC_API_KEY']}"
PAMM = "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"
POOL_DISC = bytes([241, 154, 109, 4, 17, 177, 109, 188])


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
                print("ERR", out["error"])
                continue
            return out
        except Exception:  # noqa: BLE001
            continue
    return None


def b58e(b):
    alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    n = int.from_bytes(b, "big")
    out = ""
    while n > 0:
        n, r = divmod(n, 58)
        out = alphabet[r] + out
    return "1" * (len(b) - len(b.lstrip(b"\x00"))) + (out or "")


def main():
    res = rpc(
        "getProgramAccountsV2",
        [
            PAMM,
            {
                "encoding": "base64",
                "filters": [{"memcmp": {"offset": 0, "bytes": b58e(POOL_DISC)}}],
                "limit": 120,
            },
        ],
    )["result"]["value"]["accounts"]
    authorities = set()
    programs = collections.Counter()
    pools = 0
    authority_is_pool = 0
    total = 0
    for entry in res:
        pool = entry["pubkey"]
        data = base64.b64decode(entry["account"]["data"][0])
        off = 8 + 1 + 2 + 32 + 32 + 32 + 32  # disc, bump, index, creator, mints, lp_mint
        vaults = [b58e(data[off : off + 32]), b58e(data[off + 32 : off + 64])]
        pools += 1
        for vault in vaults:
            info = rpc("getAccountInfo", [vault, {"encoding": "base64"}])["result"]["value"]
            if not info:
                continue
            tdata = base64.b64decode(info["data"][0])
            authority = b58e(tdata[32:64])
            total += 1
            authorities.add(authority)
            programs[info["owner"]] += 1
            authority_is_pool += authority == pool
    print(f"pools={pools} vault_accounts={total} distinct_authorities={len(authorities)}")
    print(f"authority==pool: {authority_is_pool}/{total}")
    print("vault owner (owning) programs:", dict(programs))


if __name__ == "__main__":
    main()
