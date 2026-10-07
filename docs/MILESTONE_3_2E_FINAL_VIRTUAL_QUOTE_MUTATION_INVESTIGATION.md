# Neurone — M3.2E: Final Virtual-Quote Mutation Investigation

Investigation only. No production change. M3.2B gate unchanged.

Statuses: `CONFIRMED`, `OBSERVED`, `INFERRED`, `HYPOTHESIS`, `UNRESOLVED`.

---

## 1. Executive Summary

The quote-only mutation behind the `previous_event_not_contiguous` exclusions is
**identified**: it is performed by the **pump.fun `SweepProtocolFee` /
`SweepCreatorFee` instruction**, confirmed directly from the on-chain Anchor
logs of a real transaction (`Program log: Instruction: SweepProtocolFee`,
emitted by the pump program `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P`).

These instructions sweep accrued protocol/creator fees out of the bonding curve
and **adjust the curve's `virtual_quote_reserves`** while leaving the token
reserves untouched — exactly the `Δvirtual_base = 0, Δvirtual_quote ≠ 0`
signature M3.2D measured. They emit **no `TradeEvent`**; their program data
(disc `742b4dbd117a482b`) is not in the published public IDLs and is not decoded
by Neurone.

**Why Neurone misses it:** the decoder consumes only `TradeEvent` / BuyEvent /
SellEvent, so a state change made outside a trade never reaches the market state.
The resulting curve state is only visible via the bonding-curve **account**
stream, which the M3.2B gate already consults but which does not reliably
corroborate the pre-state in the single-pass harness.

**Safety:** legitimate, documented-in-essence protocol fee administration — not
malicious. However it **materially changes the effective quote reserve**, so a
stale pre-state is dangerous: any armed trade must be invalidated when such a
mutation occurs. This is a state-observability requirement, not a token-safety
finding.

**Fix decision:** the instruction is identified, but the evidence does not prove
that Neurone can deterministically obtain the post-sweep curve state *before*
the next trade in the current pipeline → **INSUFFICIENT EVIDENCE** to ship a
production change; the fail-closed `UnsupportedState` boundary is retained.

## 2. Concrete Transaction Evidence

Real transaction on curve `AF3r22z5Nuz4wo2dRiE2EAt9dw7z984VQ736gpHtJ9k7`:

```
slot = 454323031
sig  = 5eZVo3ikorRqZxpvPLc25hxCt2LdishHdGbnv2qPNx8eXRvLERZyCk9jX611cqXxioPYgvca7S8iMwNmW3ssUcoU
logs:
  Program 6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P invoke [1]
  Program log: Instruction: SweepProtocolFee
  Program data: <disc 742b4dbd117a482b, 153 bytes>
  ... (repeated) ...
  Program log: Instruction: SweepCreatorFee
  Program data: <disc 742b4dbd117a482b>
  ... (repeated) ...
```

Scanning the curve's 150 most recent transactions, pump-invoking transactions
**without** a `TradeEvent` are exactly these sweeps (1 in the sampled window
because they are relatively infrequent per curve but affect many curves); the
measured `Δvirtual_base = 0, Δvirtual_quote ≠ 0` deltas (M3.2D, four curves)
match the sweep's effect.

## 3. Exact Instruction

| Field | Value |
|---|---|
| Program | pump `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P` |
| Instructions | `SweepProtocolFee`, `SweepCreatorFee` (names from on-chain Anchor logs) |
| Program-data discriminator | `742b4dbd117a482b` (153-byte payload) |
| Present in published IDL? | **No** — not in `idl/pump.json`/`pump_amm.json`/`pump_fees.json` |
| Emitted event | pump-fees-style program data only; **no `TradeEvent`** |
| Effect | sweeps accrued fees; adjusts the curve's `virtual_quote_reserves` |
| Account modified | the bonding-curve account (quote-side adjustment) |

The published public IDL does not cover these instructions, so the names are
taken from the deployed program's own logs (`CONFIRMED` for the instruction name;
the exact argument/account layout is `UNRESOLVED`).

## 4. State Transition

```
previous trade post-state            (virtual_base = B, virtual_quote = Q)
        │  SweepProtocolFee / SweepCreatorFee  (no TradeEvent)
        ▼
curve account state                  (virtual_base = B, virtual_quote = Q − fees)
        │  next pump.fun trade
        ▼
TradeEvent                           pre derived from  post − delta
                                     ≠ previous trade post  → gate rejects
```

Measured deltas (M3.2D): `Δvirtual_base = 0`, `Δvirtual_quote ∈
[−15.4 SOL, +0.13 SOL]` across sampled curves — token reserves never move.

## 5. Why Neurone Misses It

Pipeline trace:

* The sweep **transaction is delivered** by Yellowstone (it is a pump.fun
  transaction; the existing `transactions` filter includes it). `CONFIRMED`.
* The **sweep instruction is not decoded**: `src/decode` handles only
  `TradeEvent`/`BuyEvent`/`SellEvent`, and `742b4dbd117a482b` is unknown → no
  market-state update. This is outcome **D — receives the instruction/program
  but does not decode the mutation**.
* The **resulting curve state is only visible via the account stream** (the
  bonding-curve account update). The M3.2B gate already cross-checks the account
  cache; it corroborated only ~43–81% of trades, so this path is not reliable in
  the current single-pass harness (outcome **F — account stream timing**, but
  not the cause of a *validator* bug).

## 6. Safety Analysis

* **Protocol legitimacy** — `CONFIRMED`: `SweepProtocolFee`/`SweepCreatorFee` are
  ordinary protocol fee administration, documented in essence by the pump docs
  (creator/protocol fees) even though the exact instruction is absent from the
  published IDL.
* **Execution safety** — a sweep **changes the executable quote reserve without a
  trade**. A quote or armed trade computed from a pre-sweep state would be wrong.
  Therefore any armed trade on that curve must be invalidated on such a mutation,
  and a stale pre-state is unsafe — Neurone must **not** quote without observing
  it.
* **Token maliciousness** — none. This is **state observability**, not a
  token-safety finding; M3.2D's conclusion is not reversed.

## 7. Production Fix Decision

**`INSUFFICIENT EVIDENCE`**

* The mutation is deterministically *encoded* on chain and the post-sweep curve
  state is a normal bonding-curve **account write**, so a fix is *conceivable*
  (decode the sweep, or treat the bonding-curve account stream as authoritative
  and ordered before trades).
* `NOT SAFE TO IMPLEMENT` *as a full 100% support claim*: the evidence gathered
  here does not demonstrate that Neurone can **guarantee** it observes the
  post-sweep state before the next trade (the same-slot ordering and account
  delivery guarantees are unproven; the M3.2C deferred-retry test did not
  resolve the exclusions).
* Per the task rule — *"if Neurone cannot guarantee the correct pre-state it must
  remain UnsupportedState"* — the fail-closed boundary is the correct design for
  these trades today.

## 8. Implementation

None. No production behavior changed.

If the operator later authorises it, the minimal candidate is: decode the pump
`742b4dbd117a482b` sweep (or track the bonding-curve account) and apply the
`virtual_quote_reserves` change as a market-state update, then keep the existing
exact-equality gate. This must be validated for ordering guarantees first.

## 9. Validation

No change shipped, so parity is identical to M3.2B/M3.2C/M3.2D:

* All supported paths 100% exact; **0 mismatches**; **0 regressions**.
* M3.2B baseline: 1,654 exclusions (pre-fix) → 659 after the gate fix.
* M3.2D baseline: unchanged (659; ~25–30% of pump.fun SELL/token-target,
  traffic-dependent).
* M3.2E result: unchanged (no fix shipped).
* 93 deterministic tests pass; clippy/fmt clean; release build ok.

## 10. Remaining Unknowns

* The sweep instruction's exact **argument/account layout** (`UNRESOLVED`) — not
  in the published IDL; decoding it requires the Anchor IDL or reverse
  engineering.
* Whether the bonding-curve account update for a sweep is **guaranteed to be
  observed before the next trade** in a continuously running scanner
  (`HYPOTHESIS`, untested).
* Whether same-slot sweep→trade ordering can be handled deterministically.

## 11. Final Verdict

**`PARTIALLY RESOLVED`**

M3.2's remaining pump.fun exclusions are now explained at the instruction level:
they follow a **`SweepProtocolFee` / `SweepCreatorFee`** mutation of
`virtual_quote_reserves` that emits no `TradeEvent`. It is legitimate protocol
behavior, not a token-safety issue, but it is not yet deterministically
observable before the next trade in the current pipeline, so the fail-closed
`UnsupportedState` boundary is retained and no production change is shipped.

Stopping here per scope. Not proceeding to M4.
