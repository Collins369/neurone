# Neurone — M3.3: Sweep Reserve Decoding

Scope: determine whether pump.fun `SweepProtocolFee` / `SweepCreatorFee` can be
safely decoded and their **exact `virtual_quote_reserves` mutation** applied, and
implement only the smallest safe change if that mutation can be *proven*.
No M4 work, no unrelated refactors. Production runtime behavior is unchanged.

Method: read-only on-chain evidence (Solami RPC: `getSignaturesForAddress`,
`getTransaction`, `getAccountInfo`) plus the current published pump IDL, plus the
existing offline test suite. Credentials were only read from the environment and
never printed or committed.

**Bottom line:** the instruction and its emitted event are now fully decoded
(published IDL + on-chain bytes agree), but the **exact effect of the sweep on
`virtual_quote_reserves` is provably zero** — across 30 on-chain samples (both
instruction types, SOL- and USDC-quoted curves) the curve's `virtual_quote_reserves`
and `virtual_token_reserves` are byte-identical before and after the sweep. There
is therefore **no reserve mutation to apply**; a decoder that applied the swept
`amount` to `virtual_quote_reserves` would corrupt state. No production change was
made; the existing fail-closed `SWEEP → INVALIDATED → authoritative account state`
fallback is retained.

---

## A. Investigation

### A.1 Program and instruction discriminators

| Field | Value |
|---|---|
| Program | pump `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P` |
| `sweep_protocol_fee` discriminator | `0830be07b644b7e5` |
| `sweep_creator_fee` discriminator | `20f6bf3408c949ba` |
| Emitted event `SweepBondingCurveFeeEvent` discriminator | `742b4dbd117a482b` |

Both instructions are present in the **current published public IDL**
(`pump-fun/pump-public-docs → idl/pump.json`), so the discriminators, argument
list and event schema are no longer reverse-engineered:

```
sweep_creator_fee   discriminator 20f6bf3408c949ba   args: (none)
sweep_protocol_fee  discriminator 0830be07b644b7e5   args: (none)
event SweepBondingCurveFeeEvent discriminator 742b4dbd117a482b
```

This matches the M3.2E on-chain record (log text `Instruction: SweepProtocolFee`
/ `SweepCreatorFee`, program-data disc `742b4dbd117a482b`, 153-byte payload).

### A.2 Exact instruction-data layout (Anchor/Borsh)

The instruction data is **8 bytes: the discriminator only**. There are **no
arguments** — no amount, no delta, no reserve value is carried in the
instruction:

* IDL: `args: []` for both instructions.
* On-chain: every sweep instruction in every sampled transaction had
  `data_len == 8`, equal to the discriminator (e.g. `0830be07b644b7e5`,
  `20f6bf3408c949ba`).

Consequently no `amount`/delta can be decoded from the instruction itself.

### A.3 Emitted event layout (the only place an amount appears)

Each sweep logs one `SweepBondingCurveFeeEvent` (`Program data: 742b4dbd117a482b …`,
153 bytes). Borsh field order (IDL) and offsets, verified against raw bytes:

```
@0    disc = 742b4dbd117a482b        (8)
@8    timestamp      : i64           (8)
@16   mint           : pubkey        (32)
@48   bonding_curve  : pubkey        (32)
@80   quote_mint     : pubkey        (32)   -- default pubkey (111…111) for SOL pairs
@112  recipient      : pubkey        (32)   -- protocol fee recipient or creator fee recipient
@144  amount         : u64           (8)    -- the swept fee, in quote lamports/base units
@152  bucket         : u8            (1)    -- 0 = protocol, 1 = creator
```

Total = 153 bytes. The `amount` is the **fee transferred out**, not a reserve
delta (see §A.6).

### A.4 Relevant account layout (`BondingCurve`)

The single account that owns the curve state is `BondingCurve`
(disc `17b7f83760d8ac60`). Current IDL layout, with the offsets used here:

```
@0    disc                    (8)
@8    virtual_token_reserves  : u64     <- "virtual_base"
@16   virtual_quote_reserves  : u64     <- "virtual_quote" (this is the field the task targets)
@24   real_token_reserves     : u64
@32   real_quote_reserves     : u64
@40   token_total_supply      : u64
@48   complete                : bool
@49   creator                 : pubkey
@81   is_mayhem_mode          : bool
@82   is_cashback_coin        : bool
@83   quote_mint              : pubkey   (111…111 = SOL)
@115  creator_fee_bps         : u64
@123  can_edit_creator_fee    : bool
@124  is_holder_reward        : bool
@125  creator_fee             : u64      <- accrued creator fee held in the curve
@133  protocol_fees           : u64      <- accrued protocol fee held in the curve
@141  depth                   : u8  …    (later fields; older accounts truncate)
```

The `virtual_quote_reserves` field is at **offset 16**. Its value is what the
quote engine consumes (mapped to `virtual_quote_reserve` for SOL pairs).

### A.5 Whether the sweep mutates the curve directly or via another program

The sweep transaction invokes the **pump program directly** as a top-level
instruction (the only other top-level instruction is `ComputeBudget`). The pump
program then does a self-CPI (`Program 6EF8… invoke [2]`, `Program data: …`) that
emits the event and performs the fee movement. No third-party program mutates
the bonding curve. The curve's **lamports** are debited and the recipients are
credited inside the pump program; the accrued `creator_fee` / `protocol_fees`
fields are zeroed. The virtual reserves are not touched (§A.6).

### A.6 Exact transformation of `virtual_quote_reserves` (the decisive result)

For every sample the pre-state is the curve's most recent **trade** write (the
`TradeEvent` carries post-trade reserves; verified independently, see §C.0) and
the post-state is the current on-chain account immediately after the sweep:

```
pre virtual_quote_reserves  +  (effect)  ==  post virtual_quote_reserves
     effect = 0        ->  holds exactly in 30 / 30 samples
     effect = -amount  ->  holds in  0 / 30 samples
```

The sweep leaves `virtual_quote_reserves` **unchanged** (and
`virtual_token_reserves` unchanged). What it *does* change is the curve's
lamport balance and its `creator_fee` / `protocol_fees` fields:

* Curve lamports debit = the sum of the `amount`s of the curve's protocol +
  creator sweeps in that transaction (verified against the transaction's
  `preBalances`/`postBalances`).
* The recipient accounts receive exactly those amounts.
* After a sweep, `creator_fee` and `protocol_fees` read `0`; a curve that traded
  but has not been swept shows non-zero accrued fees (e.g. `creator_fee =
  453627`).

### A.7 Whether protocol and creator sweeps differ

They share one instruction shape (8-byte discriminator, no args) and one event
shape. They differ only in the event's `bucket` (`0` protocol / `1` creator),
the `recipient`, and which accrued field is zeroed (`protocol_fees` vs
`creator_fee`). **Neither changes the virtual reserves.**

### A.8 Reference transactions (real evidence)

Reference A — sweep is the curve's most recent write (unambiguous):

```
sweep tx : 3iPZksnCG9YpmfinyrcgJVQdvNVZHtX9bWtHccKXrhuGTTP5AKbGbvUwwooqYPzdfBrnqQJCpkFfPcn9Zs8K85Fn
slot     : 454473846
curve    : iGmCtY1oKkuqNuqJuWdT5gmKXbxX1gefggWevnKpiot   (mint FPbh5sobE2TLegLgaqXL7X9ijgQvjUAZQjxEfGT5pump)
last trade (pre-state): slot 454470096 SELL
    virtual_quote_reserves = 15,675,989,931
    virtual_token_reserves = 1,082,438,909,509,526
sweeps in tx on this curve:
    SweepProtocolFee amount = 5,051,648
    SweepCreatorFee  amount = 1,595,258     (total 6,646,906)
curve lamports delta in tx = -6,646,906  (== swept total)
account after (getAccountInfo):
    virtual_quote_reserves = 15,675,989,931   (== pre, unchanged)
    virtual_token_reserves = 1,082,438,909,509,526 (unchanged)
    creator_fee = 0, protocol_fees = 0
equality: 15,675,989,931 + 0 == 15,675,989,931   ✔
if the amount were applied: 15,675,989,931 - 6,646,906 = 15,669,343,025 ≠ 15,675,989,931  ✘
```

Reference B — the M3.2E signature (instruction/shape confirmation):

```
sig  : 5eZVo3ikorRqZxpvPLc25hxCt2LdishHdGbnv2qPNx8eXRvLERZyCk9jX611cqXxioPYgvca7S8iMwNmW3ssUcoU
slot : 454323031
6 sweeps over 4 curves (4× SweepProtocolFee, 2× SweepCreatorFee);
instruction data_len = 8 (disc only); events are 153-byte SweepBondingCurveFeeEvent.
```

### A.9 Note on the M3.2E attribution (kept factual, not re-litigated)

The "known fact" that sweeps mutate `virtual_quote_reserves` came from M3.2E.
Two independent observations now weigh against that attribution:

1. M3.2D's `previous_event_not_contiguous` deltas were trade-sized
   (−1.1e9, −1.5e10 lamports…) while sweep `amount`s are fee-sized
   (1.3e5 … 1.6e7 lamports) — roughly 1000× apart.
2. M3.2D's forensic window on curve `AF3r22z5…` was slots 454320420–454320508;
   the sweep M3.2E identified on that curve is at slot 454323031, ~2500 slots
   later. The sweep cannot have produced those deltas.

The true cause of `previous_event_not_contiguous` therefore remains open (it
persisted live: 258 in the 25 s `neurone validate` run). That is out of this
task's scope and is recorded as a remaining limitation (§G); it does not change
the conclusion about sweeps.

---

## B. Implementation

**No production change was made.** Every code path (`src/events.rs`,
`src/shard.rs`, `src/market.rs`, `src/quote.rs`, `src/validate.rs`) is untouched.

Reason: the task's implement-if-proven condition is *"if it can be proven …
implement the smallest safe change … their exact `virtual_quote_reserves`
mutation."* The exact mutation is proven to be **identity (0)**. There is no
nonzero reserve effect to apply, and the sweep carries no reserve value to
decode. Implementing a decoder that applies the event `amount` (or any inferred
delta) to `virtual_quote_reserves` would be **incorrect** and is exactly the
speculative decoder the task forbids.

Because the exact behavior *is* proven, the safe, minimal action is to leave the
existing fail-closed fallback in place:

```
KNOWN → (sweep observed) → INVALIDATED → (fresh authoritative account update) → KNOWN
```

This is correct for the proven identity effect: the sweep writes the curve
account (lamports + fee fields change), so the authoritative account stream
delivers the true post-sweep state, and the virtual reserves it carries are the
same as before. No stale quote can result.

Production hot-path constraints are therefore trivially satisfied (no new code,
no RPC/HTTP/DB/FS, no locks, no polling).

---

## C. Proof table

### C.0 Method and control

For each sample: pre-state = the curve's most recent `TradeEvent`
`virtual_quote_reserves` at a strictly earlier slot than the sweep; post-state =
`getAccountInfo` `virtual_quote_reserves` after the sweep (the sweep tx is the
curve's newest transaction, so no later write exists). "Decoded effect" is the
on-chain-observed delta.

Control (validates the method): in **all 30** samples the current account
`virtual_quote_reserves` exactly equals a `TradeEvent`'s `virtual_quote_reserves`
— i.e. `TradeEvent` reserves are post-trade values and the account reflects the
latest write. This is what makes "account == last trade post ⇒ no sweep effect"
valid.

Sample set: 11 distinct sweep transactions, 30 distinct curves, sizes
128,802 … 16,442,952 lamports; 30 curves received a protocol sweep and 13 a
creator sweep; 28 SOL-quoted and 2 USDC-quoted curves.

### C.1 Table (pre + decoded effect = post)

| Signature | Instruction | Pre VQ | Decoded Effect | Expected Post VQ | Actual Post VQ | Exact |
|---|---|---:|---:|---:|---:|---|
| `3iPZksnCG9Ypmfin…` | SweepCreatorFee, SweepProtocolFee | 15,675,989,931 | 0 | 15,675,989,931 | 15,675,989,931 | yes |
| `3iPZksnCG9Ypmfin…` | SweepCreatorFee, SweepProtocolFee | 17,087,223,489 | 0 | 17,087,223,489 | 17,087,223,489 | yes |
| `3iPZksnCG9Ypmfin…` | SweepProtocolFee | 27,612,420,258 | 0 | 27,612,420,258 | 27,612,420,258 | yes |
| `3iPZksnCG9Ypmfin…` | SweepProtocolFee | 2,006,079,803 | 0 | 2,006,079,803 | 2,006,079,803 | yes |
| `3iPZksnCG9Ypmfin…` | SweepProtocolFee | 14,111,499,850 | 0 | 14,111,499,850 | 14,111,499,850 | yes |
| `XyEG2Hh1YD5NwgLc…` | SweepProtocolFee | 166,768,380,365 | 0 | 166,768,380,365 | 166,768,380,365 | yes |
| `5Fheiw1TT1YVNLMc…` | SweepCreatorFee, SweepProtocolFee | 58,332,943,861 | 0 | 58,332,943,861 | 58,332,943,861 | yes |
| `5Fheiw1TT1YVNLMc…` | SweepProtocolFee | 3,564,873,415 | 0 | 3,564,873,415 | 3,564,873,415 | yes |
| `5Fheiw1TT1YVNLMc…` | SweepProtocolFee | 31,498,410 | 0 | 31,498,410 | 31,498,410 | yes |
| `5Fheiw1TT1YVNLMc…` | SweepCreatorFee, SweepProtocolFee | 33,998,055,938 | 0 | 33,998,055,938 | 33,998,055,938 | yes |
| `2o9KnY5kVffVoodK…` | SweepCreatorFee, SweepProtocolFee | 1,217,412,235 | 0 | 1,217,412,235 | 1,217,412,235 | yes |
| `2o9KnY5kVffVoodK…` | SweepProtocolFee | 198,664,412,702 | 0 | 198,664,412,702 | 198,664,412,702 | yes |
| `5vm2pmAWBjFXfEJB…` | SweepCreatorFee, SweepProtocolFee | 2,219,770,325 | 0 | 2,219,770,325 | 2,219,770,325 | yes |
| `5vm2pmAWBjFXfEJB…` | SweepCreatorFee, SweepProtocolFee | 5,618,012,006 | 0 | 5,618,012,006 | 5,618,012,006 | yes |
| `3LWtdQzmLYV2rUYb…` | SweepCreatorFee, SweepProtocolFee | 13,652,623 | 0 | 13,652,623 | 13,652,623 | yes |
| `4ZaVYvJEvDnTMFss…` | SweepCreatorFee, SweepProtocolFee | 41,402,720,029 | 0 | 41,402,720,029 | 41,402,720,029 | yes |
| `4ZaVYvJEvDnTMFss…` | SweepProtocolFee | 4,932,362,890 | 0 | 4,932,362,890 | 4,932,362,890 | yes |
| `4ZaVYvJEvDnTMFss…` | SweepProtocolFee | 3,461,406,122 | 0 | 3,461,406,122 | 3,461,406,122 | yes |
| `33kGWiuTA3AmxSrL…` | SweepCreatorFee, SweepProtocolFee | 13,111,316,258 | 0 | 13,111,316,258 | 13,111,316,258 | yes |
| `33kGWiuTA3AmxSrL…` | SweepProtocolFee | 28,918,731,304 | 0 | 28,918,731,304 | 28,918,731,304 | yes |
| `33kGWiuTA3AmxSrL…` | SweepProtocolFee | 17,718,667,059 | 0 | 17,718,667,059 | 17,718,667,059 | yes |
| `33kGWiuTA3AmxSrL…` | SweepProtocolFee | 73,427,186,713 | 0 | 73,427,186,713 | 73,427,186,713 | yes |
| `33kGWiuTA3AmxSrL…` | SweepProtocolFee | 5,690,516,925 | 0 | 5,690,516,925 | 5,690,516,925 | yes |
| `NJWVktabavcKHSoM…` | SweepCreatorFee, SweepProtocolFee | 805,329,562 | 0 | 805,329,562 | 805,329,562 | yes |
| `NJWVktabavcKHSoM…` | SweepCreatorFee, SweepProtocolFee | 3,542,920,968 | 0 | 3,542,920,968 | 3,542,920,968 | yes |
| `NJWVktabavcKHSoM…` | SweepProtocolFee | 3,293,136,353 | 0 | 3,293,136,353 | 3,293,136,353 | yes |
| `25oDDjWZWegf5dAP…` | SweepProtocolFee | 1,823,603,214 | 0 | 1,823,603,214 | 1,823,603,214 | yes |
| `3JhW9Ch9JiYcHX14…` | SweepCreatorFee, SweepProtocolFee | 5,974,318,579 | 0 | 5,974,318,579 | 5,974,318,579 | yes |
| `3JhW9Ch9JiYcHX14…` | SweepProtocolFee | 1,621,872,980 | 0 | 1,621,872,980 | 1,621,872,980 | yes |
| `3JhW9Ch9JiYcHX14…` | SweepProtocolFee | 53,908,279,130 | 0 | 53,908,279,130 | 53,908,279,130 | yes |

Full raw per-sample data (curve, signatures, slots, amounts, lamport deltas,
quote mint, fee fields) is stored in `research/sweep/rows.json`.

### C.2 Falsification of the "apply the amount" hypothesis

For every row, `pre − swept_total ≠ post` (the swept amounts are 128,802 …
16,442,952 lamports; the observed quote delta is 0). The event `amount` is the
fee moved out of the curve's **lamports / fee fields**, not a reduction of the
virtual quote reserve.

---

## D. Tests

No new production behavior was added, so no new tests were required by the task's
"if implemented" clause. All existing focused and regression tests pass
unmodified (see §E). The investigation itself is reproducible from the scripts in
`research/sweep/` against a live RPC.

---

## E. Validation

| Command | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --all-targets --all-features -- -D warnings` | clean (0 warnings) |
| `cargo test --all-targets --locked` | **106 passed, 0 failed, 1 ignored** |
| `cargo build --release --locked` | ok |
| `cargo test --test live_solami --locked -- --ignored --nocapture` | **1 passed** (authenticated live stream, live decode/quote) |
| `NEURONE_VALIDATE_SECONDS=25 ./target/release/neurone validate` | parity unchanged: 6,971 / 6,971 supported samples exact, **0 mismatch, 0 errors**; `reserve_mutations_seen=5`; unsupported 330 all classified (`no_previous_event_observed=72`, `previous_event_not_contiguous=258`) |

Test breakdown: lib 49 · architecture 8 · decode_and_state 6 · failed_transaction
4 · ingest_reconnect 1 · live_solami 0 (1 ignored) · m3_parity 12 · mutation 8 ·
quote_parity 5 · real_fixtures 7 · reserve_state 6 ⇒ 106 passed / 0 failed.

Live sweep observation (read-only RPC, the actual evidence for §A–C): 11 distinct
sweep transactions decoded, 30 swept curves verified, both instruction types,
SOL- and USDC-quoted curves; no credentials exposed.

---

## F. Files Changed

```
docs/M3_3_SWEEP_RESERVE_DECODING_REPORT.md   (new: this report)
research/sweep/                          (new: read-only investigation scripts + raw evidence)
  rpc.py                                (read-only RPC + base58 helpers)
  final2.py                             (IDL-faithful sweep/trade/account decoder + sample scan)
  rows.json                             (30-sample proof dataset)
```

**No `src/` file and no `tests/` file was modified.** `docs/task.md` was already
modified in the working tree before this task began and was not touched.

---

## G. Remaining Limitations

1. **No sweep reserve decoder exists because there is no sweep reserve effect.**
   The sweep is provably a no-op on `virtual_quote_reserves`; this is the
   finding, not a gap.
2. **Spurious conservative invalidations remain.** `src/events.rs` still sets
   `has_reserve_mutation` on sweep log lines and `src/shard.rs` still invalidates
   the touched markets, even though the reserves do not change. This is
   fail-closed (safe) but costs availability (a swept market is unquotable until
   its next authoritative account write). Relaxing it would be a production
   behavior change beyond this task's proven "apply the mutation" objective and
   is intentionally left untouched for explicit authorization.
3. **`previous_event_not_contiguous` is still unexplained.** It persists live
   (258 exclusions in 25 s). The M3.2E sweep attribution does not hold (§A.9);
   the real cause is a different, out-of-scope investigation.
4. **Detector is log-text based.** Detection still keys on
   `Instruction: SweepProtocolFee` / `SweepCreatorFee` log text, not on the
   discriminator; a program log change would need re-validation.
5. **Sample is bounded** (30 curves / 11 transactions at one slice of time).
   The mechanism (fees held as curve lamports + `creator_fee`/`protocol_fees`
   fields; virtual reserves untouched) makes a broad counterexample implausible,
   but the scan is not exhaustive.

---

## H. Verdict

`SWEEP DECODING NOT SAFE TO IMPLEMENT YET`

The instruction and its event are fully decoded (published IDL and on-chain
bytes agree; discriminators, zero-argument data layout, 153-byte
`SweepBondingCurveFeeEvent` layout all pinned). But the premise that the sweep
mutates `virtual_quote_reserves` is **contradicted** by direct on-chain evidence:
30/30 swept curves show `virtual_quote_reserves` and `virtual_token_reserves`
byte-identical before and after both `SweepProtocolFee` and `SweepCreatorFee`;
the swept `amount` is a lamport/fee movement, not a reserve delta. Applying any
reserve change on a sweep would therefore be provably wrong, so no decoder is
implemented and the existing fail-closed
`SWEEP → INVALIDATED → authoritative account state` fallback is retained.

Answer to the task's final question — *"Can Neurone safely understand and apply
Pump.fun fee-sweep reserve mutations directly?"*: Neurone can now fully
**understand** the sweep, but there is **no reserve mutation to apply**; doing so
would be unsafe/incorrect. Therefore: do not guess — keep the safe fallback.
