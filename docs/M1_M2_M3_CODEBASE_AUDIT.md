# Neurone — M1/M2/M3 Codebase Audit

**Type:** read-only engineering audit. No source, configuration, test, or
existing report was modified. This document is the only artifact produced.

**Method:** every claim below was checked against the *current repository code*
and *commands actually run in this audit*, not against the milestone reports.
Reports are treated as claims to verify.

**Audit host:** `/home/xion/neurone`, rustc 1.99 (stable), 1 live Solami
credential present in `.env` (used only for read-only stream tests; never
printed, logged, or committed).

---

## 1. Executive Summary

The repository is a **genuine, working M1–M3 implementation**, not a mock. The
core claims survive skepticism: a real Solami Yellowstone client, a deterministic
parallel sharded market-state engine with no global lock, real protocol decoders
validated against mainnet fixtures, an exact-integer quote engine, and a
conservative mutation-aware invalidation path. This audit independently
reproduced live reception, live decoding, and live protocol parity.

However, the code is **not frictionless for M4**. Three concrete gaps are
substantiated by source inspection and are not all reflected in the reports:

1. **Mutation invalidation is effectively terminal under the default runtime
   configuration.** The M3.3 invalidation is cleared *only* by a fresh
   authoritative **account** update, but the default config does not subscribe
   bonding-curve accounts, and a later `TradeEvent`/swap does **not** clear the
   flag. A pump.fun market that sees one fee sweep therefore stays
   `Invalidated` (unquotable) forever in the shipped `neurone run` path.
2. **Decoded swaps/creates from *failed* transactions are applied to state.**
   `TransactionUpdate.success` is computed but never consumed; the shard does
   not gate on it. A reverted transaction that still carries a `Program data:`
   event can move reserves / create a market.
3. **The pre-state corroboration gate (`UnsupportedState`) lives only in the
   validation harness (`src/validate.rs`), not in the runtime pipeline.** M4
   will have to re-implement the equivalent "was this trade's pre-state proven"
   logic before it can quote safely.

| Milestone | Verdict (this audit) |
|---|---|
| M1 — Foundation / Yellowstone | **PASS** (live reception now independently verified) |
| M2 — Real Market State | **PASS WITH LIMITATIONS** (decoders + exact quote engine real; quote engine not yet consumed by production) |
| M3 — Parallel State / Live Correctness | **PASS WITH LIMITATIONS** (100% exact on every supported path; pump.fun state-dependent residual correctly excluded; M3.3 implemented with a terminal-invalidation limitation) |

**Overall readiness: `READY FOR M4 WITH LIMITATIONS`.** See §11.

---

## 2. Repository / Architecture Reviewed

Read in full (source of truth for this audit):

| Module | Role | Notes |
|---|---|---|
| `src/main.rs`, `src/lib.rs` | entry point, `run`/`bench`/`check`/`validate` commands, module wiring | fine |
| `src/runtime.rs` | supervisor, graceful shutdown, task join | cooperative `watch`-based shutdown |
| `src/config.rs`, `config/*.toml` | precedence env>toml>default; eager pubkey validation; secret from env only | `deny_unknown_fields` |
| `src/error.rs`, `src/shutdown.rs`, `src/clock.rs`, `src/hash.rs` | typed errors, cancellation, monotonic clock, FNV-1a shard hash | |
| `src/events.rs` | protobuf → compact normalized events; program-stack event attribution; sweep detection | hot boundary |
| `src/decode/{mod,borsh,pda,pumpfun,pump_amm}.rs` | Anchor account/event decoders; PDA derivation | pure functions |
| `src/market.rs` | incremental `MarketState`, `Ratio`, `VolumeWindow`, `ReserveState`, invalidation | continuous state |
| `src/quote.rs` | exact integer quote engine + reconciled parity predictions + SDK fee primitives | no float |
| `src/shard.rs`, `src/engine.rs` | per-shard owned state, deterministic routing, broadcast, replay ring | parallel |
| `src/telemetry.rs` | lock-free atomic counters + fixed-bucket histograms + reporter task | never blocks |
| `src/ingest/{solami,simulated}.rs` | live Yellowstone client + reconnect; deterministic offline source | `Connector` trait |
| `src/bench.rs` | offline throughput / decode / quote microbenches | |
| `src/validate.rs` | live parity research harness (account cache + corroboration gate) | not the runtime path |
| `tests/*.rs` | architecture, decode/state, reconnect, real fixtures, reserve state, quote parity, m3 parity, mutation, live | |

Blueprint and every historical milestone report in `docs/` were read as claims
to verify. `NEURONE_BLUEPRINT.md` was not modified.

---

## 3. M1 Audit — Foundation / Yellowstone

### Requirements vs code

| M1 requirement | Code evidence | Assessment |
|---|---|---|
| Cargo project / runtime entry | `Cargo.toml`, `main.rs` (run/bench/check/validate), multi-thread tokio runtime sized by config | PASS |
| Configuration | `config.rs`: env > TOML > default, `deny_unknown_fields`, eager 32-byte base58 validation of every program/address before opening a socket | PASS |
| Error model | `error.rs`: coarse stage-typed `Error` enum; fail-closed intent | PASS |
| Logging / telemetry | `tracing` + `telemetry.rs` atomic counters/histograms, separate reporter task | PASS |
| Runtime lifecycle | `runtime.rs`: `select!` on Ctrl-C vs ingest exit, `shutdown.trigger()`, drain shards + reporter, join, final snapshot | PASS |
| Graceful failure | shard loop drains queued messages on shutdown; router surfaces closed channels as errors | PASS |
| Yellowstone connection/auth/TLS | `ingest/solami.rs::TonicConnector` / `build_client`: `GeyserGrpcClient`, `.x_token(Some(token))`, `ClientTlsConfig::with_native_roots()`, `http2_keep_alive_interval(20s)` | PASS |
| Subscription construction | `build_subscribe_request`: accounts (opt-in, optional memcmp + data-slice), txs (`vote=false`, `account_include=programs`), slots (`interslot_updates`), blocks-meta, `commitment` | PASS |
| Processed commitment | default `Commitment::Processed`; `to_proto()` maps to `CommitmentLevel::Processed` | PASS |
| Reconnect + backoff | reconnect loop, exponential backoff `500ms → ×2 → cap 30000ms`, reset on success | PASS |
| Stale-stream detection | per-stream `stale` timer reset on every update; timeout forces `StreamLoop::Ended` | PASS |
| Replay / from-slot | `from_slot = last_slot+1` when `replay_from_last_slot`; watermark dropped if a replay subscribe fails | PASS |
| Malformed/unexpected updates | `normalize()` returns `Ignored` (counted `invalid_events`) or `Keepalive`; never panics | PASS |

### Normalization

Raw `SubscribeUpdate` is converted exactly once into owned, bounded internal
events (`Slot`, `Account`, `Transaction`, `BlockMeta`); protobuf does not travel
downstream. Bounds are enforced: `MAX_TX_KEYS = 64`, `MAX_DECODED_EVENTS = 16`,
`MAX_PROGRAM_DATA_LEN = 2048`, signature/keys deduped+ordered. Program
attribution uses an invocation-stack walk over `Program … invoke/success/failed`
log lines, and only decodes `Program data:` payloads emitted by a known venue
program (`is_venue_program`) — this fixes the real M3 "foreign event collision"
bug and is genuinely present in code.

### Parallel routing

`engine.rs`/`shard.rs` implement `shard = fnv1a_64(pubkey) % num_shards` over a
fixed hash (never `std` `RandomState`). Each shard is an independent tokio task
that **owns** its `HashMap<MarketKey, MarketState>`; there is no shared map and
no global lock (grep for `Mutex`/`RwLock` in `src/` finds none outside the
`ingest_reconnect` test's sink bookkeeping). A transaction touching several
shards is grouped per shard and delivered once per shard. Chain-global events
are broadcast so each shard keeps its own slot watermark. Per-market ordering
is preserved because the router is a single task that sends events for a key to
that key's shard channel in arrival order.

### M1 tests

* **Genuinely integration/live:** `tests/architecture.rs` exercises the *real*
  engine (real tasks, real channels) — determinism, deterministic+balanced
  routing, per-market ordering under interleaving, replay non-corruption,
  512-market independent advance, tx-does-not-create-market, global broadcast,
  clean shutdown. `engine::tests::shards_run_as_independent_tasks` proves
  concurrency via a shard-sized barrier (a serial design would deadlock).
* **Transport-boundary mock only:** `tests/ingest_reconnect.rs` mocks only
  `Connector::subscribe`; the real reconnect loop, normalizer, router, shards
  and telemetry are exercised.
* **Live:** `tests/live_solami.rs` (ignored by default) — **verified passing in
  this audit** (see §9).

### M1 verdict: **PASS**

The M1 report under-claimed: it stated live reception was "BLOCKED" for lack of
a credential. In this audit a live credential was present and the live test
passed, so live reception is now confirmed. No M1 defect was found.

---

## 4. M2 Audit — Real Market State

### Market decoding

Two venues are decoded from official IDLs, all as pure functions over bytes:

* **pump.fun** (`decode/pumpfun.rs`): `BondingCurve` account (disc
  `[23,183,248,55,96,216,172,96]`, 49→125-byte layouts, trailing fields
  optional), `TradeEvent`, `CreateEvent`, and the bonding-curve PDA
  (`find_program_address(["bonding-curve", mint], pump)`).
* **pump.swap** (`decode/pump_amm.rs`): `Pool` account (disc
  `[241,154,109,4,17,177,109,188]`) and `BuyEvent`/`SellEvent`.

All integer reads are little-endian via a truncation-safe `Reader` (returns
`None` instead of panicking). Decoders read a validated prefix and stop, so
newer trailing fields are ignored rather than misread. A `PDA` implementation
hashes `seeds||bump||program_id||"ProgramDerivedAddress"` and requires an
off-curve result — the standard algorithm, pinned against a documented vector
and (in the fixtures test) against a real mainnet address.

Key evidence that the layouts are real, not invented:
`tests/real_fixtures.rs` decodes real 125-byte `BondingCurve`, real
`TradeEvent`, real `Pool`, and real pump.swap buy/sell event bytes, and asserts
`bonding_curve("DdtKUh…pump") == 112heubZsHqS…` (a real on-chain PDA).

### Market state and where fields are populated

`MarketState` is updated **incrementally** (never rebuilt): `apply_account`
(identity/owner/reserves, `write_version`-guarded), `apply_swap` (reserves,
volume, trade counts, fee bps), `apply_create`, `apply_touch`, `note_slot`.
Reserve provenance is explicit: `reserves_known`, `last_reserve_slot`,
`last_reserve_timestamp`, `last_reserve_signature`, plus `virtual_*` reserves
and `state_version`/`invalidated_at_slot` (M3.3). Fields are populated from
decoded events; the audit traced each field to a writer and found no field that
is present but never written.

Notable honest modelling choices: prices are **exact integer ratios** in raw
units (no decimals read → no UI scaling), and a zero reserve side yields
`None`/refusal rather than a `0` price.

### State-update path

```text
Yellowstone → normalize (events.rs) → decode account/event → route by market key
            → shard (owned map) → MarketState::apply_account / apply_swap / apply_create
```

Confirmed continuous: a market is created once and mutated thereafter; the
`VolumeWindow` is a bounded sparse per-slot ring (`DEFAULT_VOLUME_BUCKETS =
1024`, evicting oldest). Reserve ordering is guarded
(`!reserves_known || slot >= last_reserve_slot`), so an older event cannot
regress newer reserves.

### Quote/state relationship

`quote.rs` computes with `u128`/`i128` checked arithmetic, floor/ceil direction
made explicit, no floating point. `quote()` gates **first** on
`MarketState::reserve_state(current_slot, stale_slots)` and refuses
`Unknown`, `Stale`, and `Invalidated` before touching venue math — so
unproven state cannot be quoted. Deterministic error variants cover zero input,
zero reserves, insufficient liquidity, invalid fee, overflow, unsupported
venue/instruction/state.

**Important boundary:** `quote()` is **not** called from the runtime pipeline;
its only non-test call site is `src/bench.rs`. The runtime in this milestone
maintains state only. This is acceptable for M2/M3 (arming is M4+) but means
the exact quote engine's gating is, today, exercised only by tests/bench.

### M2 tests

* Real fixtures: 7 tests, incl. the mainnet PDA check and account⇄trade reserve
  agreement.
* State: `decode_and_state.rs` (6) — venue separation, volume across slots,
  replay idempotence, deterministic sequences, create-event seeding, degenerate
  reserves.
* Reserve state: `reserve_state.rs` (6) — unknown/stale, first-swap establishes,
  newer replaces, older cannot overwrite, replay idempotent, pools independent.
* Quote parity: `quote_parity.rs` (5) — exact pump.swap sell ×2, exact pump.fun
  sell, bounded pump.swap buy residual, unknown-reserves refusal.
* Live: `live_solami.rs` (ignored) — verified passing, plus a real live quote.

### M2 verdict: **PASS WITH LIMITATIONS**

Limitations are real and honestly reported: pump.swap reserves are only known
after the first observed swap (the `Pool` account stores none) and the default
config does not subscribe accounts, so a fresh pool is `Unknown` until a swap
is seen. The quote engine exists and is correct but is not yet wired to a
production consumer.

---

## 5. M3 Audit — Parallel State / Live State Correctness

### A. Parallel state architecture

Verified in §3: deterministic shard selection over a fixed hash; per-shard
owned maps; no global market lock; a transaction touching N shards is delivered
once per shard; one busy market cannot stall unrelated markets (separate
tasks/channels; proven by the barrier test and by `shard_activity` being
non-zero on every shard in live runs). `async` here is genuine parallelism:
each shard is its own task on a multi-thread runtime. The blueprint's "many
independent state machines + minimal synchronization" holds.

### B. State ordering

The engine relies on: the router being a single task (arrival order), FIFO
per-shard channels, and per-field guards in `MarketState`:

* accounts: rejected when `write_version <= self.write_version`;
* swaps: reserves only move when `!reserves_known || slot >= last_reserve_slot`;
* mutations: `invalidated_at_slot = max(existing, new)`; cleared only by an
  account update at `slot >= invalidated_at_slot`;
* transactions: bounded per-shard signature ring for replay protection.

**Finding (report vs code):** the M3.3 report §3 states that per-market
ordering uses `slot` **plus the transaction index** `SubscribeUpdateTransactionInfo.index`.
The code stores `TransactionUpdate.index` but **never reads it** (grep: the only
references are the struct field and its assignment in `normalize`). Ordering is
by slot only. This is an overclaim in the report; the code is not wrong, but the
"index" ordering key is not implemented.

**Residual ordering concern (LOW):** events are ordered by *arrival* at the
router, not by chain order. For the same market the guard fields (write_version,
slot) make regressions safe, but a same-slot interleaving of a sweep and a swap
is resolved by arrival order; the code's fail-closed default (invalidate) is the
safe outcome.

### C. State freshness / validity

`ReserveState` = `{Unknown, Known, Stale, Invalidated}`.

```
reserve_state():
  invalidated_at_slot.is_some()            -> Invalidated
  else !reserves_known                     -> Unknown
  else current_slot - last_reserve_slot>N  -> Stale
  else                                     -> Known
```

`quote()` refuses everything except `Known`. The intended lifecycle
`KNOWN → mutation → INVALIDATED → fresh authoritative update → KNOWN` is
implemented **only across account updates**. This is the crux of finding #1:

* `MarketState::apply_swap` does **not** clear `invalidated_at_slot`.
* `MarketState::apply_account` clears it only when `ev.slot >= invalidated_at_slot`.
* The default `config/default.toml` sets `account_programs = []` and
  `account_addresses = []`, so the production `neurone run` path receives **no**
  bonding-curve account updates.

Consequence: in the shipped runtime configuration a pump.fun market invalidated
by a sweep can never return to `Known`. `tests/mutation.rs` proves the *code
path* works (it injects account events directly), but no test exercises the
runtime config that leaves the account stream absent. This is fail-closed (no
unsafe trade) but is a functional/availability defect for M4 and is not called
out as such in the report.

### D. M3.2 validation gate

Implemented in `src/validate.rs`:

* `CurveCache`/`CurveHistory` keep a slot-ordered ring of
  `(slot, virtual_base, virtual_quote)` per bonding curve, plus a startup count.
* For a pump.fun SELL / token-target buy, the derived pre-state is corroborated
  by either (a) a contiguous previous event on the same curve
  (`prev.slot <= trade.slot && prev.post == derived_pre`) or (b) a cached
  account state strictly older than the trade (`slot < trade.slot`) equal to the
  derived pre-state.
* Uncorroborated trades return `QuoteError::UnsupportedState` and are counted,
  with a classified reason (`no_previous_event_observed`,
  `previous_event_not_contiguous`, `no_account_state_cached`,
  `account_state_not_older_than_trade`, `account_state_mismatch`).
* `evaluate()` also runs a `coherent()` guard rejecting physically-impossible
  payloads (`token_amount > 1e15`, zero reserve with a non-zero trade) as
  `ZeroReserves` rather than as formula mismatches.

The gate cannot "accidentally quote an unproven state": uncorroborated samples
are never evaluated. **But it lives in the research harness**, not the runtime
pipeline (finding #3) — the runtime has no gate because it does not quote yet.

### E. M3.3 mutation handling

* **Detection:** `events.rs::detect_reserve_mutation` scans
  `meta.log_messages` for `Instruction: SweepProtocolFee` / `SweepCreatorFee`
  and sets `TransactionUpdate.has_reserve_mutation`.
* **Invalidation:** `shard.rs` invalidates every tracked market in the
  transaction's `keys` when the flag is set, via `MarketState::invalidate(slot)`
  (bumps `state_version`, sets `invalidated_at_slot = max(...)`), and increments
  `reserve_invalidations` telemetry.
* **Refusal:** `reserve_state()` returns `Invalidated`; `quote()` returns
  `QuoteError::StateInvalidated`.
* **Scope:** invalidation is per key on the shard-local map — a mutation for one
  market does not stall unrelated markets.

Live evidence reproduced in this audit: `reserve_mutations_seen = 1` in a 25 s
`neurone validate` run (the detection fires on real traffic).

The payload is deliberately **not** decoded (absent from public IDLs); the
resulting reserve value is not applied. As noted in §5.C, re-validation depends
entirely on an account update that the default config does not deliver.

Two smaller code-quality observations:

* `detect_reserve_mutation` is **not** program-gated: it scans *all* log lines,
  so an unrelated program logging the same text could trigger invalidation
  (fail-closed, low impact).
* It also runs on failed transactions (see F/finding #2), so a reverted sweep
  needlessly invalidates a market (fail-closed, low impact).

### F. Fail-closed behavior

The dangerous patterns named in the task were searched for:

```text
unknown state → assume → quote → trade      NOT present (quote gates on Known)
stale state   → continue with old quote      NOT present (quote gates on Known/Stale)
mutation      → silently ignore              NOT present (invalidates + counts)
```

No path was found where the code quotes `Unknown`/`Stale`/`Invalidated` state.
The one genuine fail-*open* risk found is orthogonal to reserve freshness:
**failed transactions are not filtered** (finding #2, §6).

### M3.3 performance / parallelism

* No global mutex/RwLock in `src/`.
* No blocking I/O on the hot path: the only `std::fs` read is the startup config
  load in `config.rs`; no HTTP/RPC/DB in `src/` (RPC exists only in
  `research/*.py`).
* Mutation handling is a bounded `log_messages` substring scan (O(1) per tx) +
  an O(keys) invalidation over shard-local maps — no scan of unrelated markets.
* `events.rs` caps protobuf retention: only bounded, owned values leave the
  ingestion boundary.
* Telemetry recording is atomic-only (no channel/mutex/I-O on the record path).

### M3 tests

`m3_parity.rs` (12) pins the reconciled integer rules (pump.swap sell exact,
signed virtual reserve direction, negative effective reserve rejected, `−1` on
exact-in, token-target forms, strict classification, SDK fee primitives).
`mutation.rs` (6) covers invalidation, refusal, re-validation, stale-update
non-revalidation, repeated mutation, and log-based detection. Live parity:
**reproduced in this audit** (§9).

### M3 verdict: **PASS WITH LIMITATIONS**

PumpSwap (sell / `buy_exact_in` / token-target) and pump.fun exact-in buys are
**100% exact on live data** (independently reproduced). Pump.fun SELL /
token-target are 100% exact on *corroborated* samples and correctly *excluded*
(`UnsupportedState`) otherwise. M3.3 is implemented but carries the
terminal-invalidation limitation under the default config.

---

## 6. Cross-Milestone Findings

* **One state model, two consumers.** The runtime maintains `MarketState`; the
  validation harness separately maintains `CurveCache`. They encode overlapping
  truths (reserve provenance, freshness) with different code. M4 must converge
  on one (the harness's corroboration logic needs to move into the pipeline or a
  shared module).
* **Freshness is slot-based only.** `reserve_stale_slots` (default 150 ≈ 60 s)
  is the sole freshness gate; there is no wall-clock or event-age gate. This is
  consistent with the blueprint (no RPC polling) but means a market with old,
  never-refreshed reserves is only *stale*, not *invalid*.
* **`success`/`is_vote`/`index` are carried but unused.** They are dead
  payload today; `vote=false` is enforced server-side, but `failed` is not
  filtered, and `success` is never checked before applying decoded state.
* **The quote engine is ahead of the pipeline.** `quote()`, the SDK fee
  primitives, and the parity predictors are complete and tested, but nothing in
  the running system consumes them. This is the expected M3→M4 handoff, not a
  defect.

---

## 7. Report-vs-Code Discrepancies

| Milestone | Report claim | Code evidence | Test evidence | Actual assessment |
|---|---|---|---|---|
| M1 | "live verified; reconnect/backoff/stale/replay implemented" | Code matches; `ingest/solami.rs` real client + loop | `live_solami` passed this audit; reconnect tested | PASS |
| M1 | "live event reception BLOCKED" | Code ready; credential present this run | live test passed | **Underclaim** — M1 is better than reported |
| M2 | "both venues decoded; live verified" | Decoders real, fixture-pinned | fixtures + live pass | PASS WITH LIMITATIONS |
| M2 | "quote engine exact" | `quote.rs` real, gated on freshness | parity + unit tests | PASS (but not on a production path) |
| M3 | "PumpSwap 100%, pump.fun exact-in 100%" | formulas in `quote.rs` | reproduced live this audit | PASS |
| M3 | "pump.fun SELL/token-target 100% of supported, else excluded" | gate in `validate.rs` | reproduced live (0 mismatch) | PASS (of supported) |
| M3.3 | "ordering uses slot **+ transaction index**" | `index` stored, **never read** | none | **Overclaim** — slot only |
| M3.3 | "market resumes normally once a fresh authoritative state arrives" | true only via account updates; default config has none | `mutation.rs` injects accounts directly | **Overclaim** in the default runtime config |
| M3.3 | "invalidation is O(keys), no global lock" | `shard.rs` invalidation loop | — | accurate |

**Overclaims:** M3.3's transaction-index ordering; M3.3's implicit assumption
that re-validation is available in the shipped runtime.

**Underclaims:** M1's live status (now verified). The M3.2A/B/C/D/E reports are
internally consistent and appropriately hedged (`PARTIALLY RESOLVED`); no
overclaim found in them.

**Genuine gaps (not M4 features):** (i) invalidation not recoverable under
default config; (ii) failed-transaction events applied; (iii) corroboration gate
not in the runtime.

---

## 8. Critical Findings

### HIGH

**H1 — Mutation invalidation is terminal under the default configuration.**
`config/default.toml` does not subscribe bonding-curve accounts
(`account_programs=[]`), and `apply_swap` never clears `invalidated_at_slot`.
Once a pump.fun market is invalidated by a fee sweep in `neurone run`, no
subsequent event can return it to `Known`. It is fail-closed (safe) but makes
pump.fun state-dependent quoting permanently unavailable for swept markets in
the shipped path. *Evidence:* `market.rs::apply_account`/`apply_swap`/`invalidate`,
`shard.rs`, `config/default.toml`. *Not covered by any test or report.*

### MEDIUM

**M1 — Failed-transaction events are applied to state.** `TransactionUpdate.success`
is computed (`events.rs`) but never consumed; `shard.rs` applies `swaps`,
`creates`, and invalidation regardless. A reverted transaction that still
carries a `Program data:` event (e.g., a later instruction in the same tx fails)
can move reserves or create a market. *Evidence:* grep shows no read of
`success`; `shard.rs` `EventKind::Transaction` arm. *Recommendation:* filter on
`success` before decoding/applying.

**M2 — Pre-state corroboration gate is not in the runtime.** `UnsupportedState`
and the contiguity/account cross-check live only in `validate.rs`. M4 must
re-implement equivalent "proven pre-state" gating before quoting pump.fun
SELL/token-target. *Evidence:* `validate.rs` only; `quote::classify` used
nowhere in the `src/` runtime besides `validate.rs`.

**M3 — `TransactionUpdate.index` is documented as an ordering key but is
unused.** Ordering is slot-only. *Evidence:* grep; M3.3 §3.

### LOW

**L1 — `detect_reserve_mutation` is not program-gated and ignores `success`.**
Scans all log lines and failed transactions; fail-closed but can cause spurious
invalidation.

**L2 — `MAX_TX_KEYS = 64` can drop relevant keys.** Markets beyond the 64th
account key are neither touched nor invalidated. Bounded/defensive, but a real
coverage edge.

**L3 — `MarketState::spot_price_raw` / `executable_price` are ungated
primitives.** They will compute from `0`/stale fields if called without checking
`reserve_state()`; only `quote()` is gated. A future consumer could misuse them.

### INFORMATIONAL

**I1 — `quote()` is not on any production path** (bench/tests only). Expected
at M3; a note for M4 wiring.

**I2 — Telemetry percentiles use `f64`.** Correct: this is the cold/telemetry
path; the protocol/trading math is integer-only.

**I3 — No `TODO`/`FIXME`/`unimplemented` in `src/` or `tests/`** (grep clean).
`unwrap`/`expect` occur only in tests and one guarded invariant
(`market.rs:136 expect("just pushed")` immediately after a `push_back`).

**I4 — `validate.rs::_unused` is a dead function** (`#[allow(dead_code)]`); the
report says "unused here" — accurate, harmless.

---

## 9. Tests Actually Run

All commands executed in `/home/xion/neurone` with `--locked`. No source,
config, test, or existing report was modified; `target/` is gitignored.

| Command | Result |
|---|---|
| `cargo fmt --check` | **clean** |
| `cargo clippy --all-targets --all-features -- -D warnings` | **clean** (0 warnings) |
| `cargo build --release --locked` | **ok** |
| `cargo test --all-targets --locked` | **99 passed, 0 failed, 1 ignored** |

`cargo test --all-targets --locked` breakdown (exact):

```
lib unit tests                48 passed
main unit tests                0 passed
tests/architecture.rs          8 passed
tests/decode_and_state.rs      6 passed
tests/ingest_reconnect.rs      1 passed
tests/live_solami.rs           0 passed, 1 ignored
tests/m3_parity.rs            12 passed
tests/mutation.rs              6 passed
tests/quote_parity.rs          5 passed
tests/real_fixtures.rs         7 passed
tests/reserve_state.rs         6 passed
                         total 99 passed / 0 failed / 1 ignored
```

| Live command (read-only network) | Result |
|---|---|
| `cargo test --test live_solami --locked -- --ignored --nocapture` | **1 passed** in 4.4 s |
| `NEURONE_VALIDATE_SECONDS=25 ./target/release/neurone validate` | see below |

Live test output (abridged): connected to Solami (default endpoint, `x-token`),
`events_received=13`, `events_decoded=47` (pump.fun 8 / pump.swap 39),
`active_market_states=39`, `worker_threads=3`, `shard_activity` non-zero on all
8 shards, `decode_rejected=0`, `stale_events=0`, `reconnects=0`; a **live
pump.swap sell quote** was produced:

```
PumpSwap sell 100000 -> gross 356, fee 1, net 355
```

Live parity (`validate`, 25 s, release):

```
connected=true updates=39796 swaps=12369 reserve_mutations_seen=1
curve_updates=2228 curves_cached=196 acct_corroborations=2215 acct_corroborated=1803

venue/instruction            samples  exact  mismatch  errors  unsupported
pumpfun/buy_exact_in             471    471         0       0          0
pumpfun/buy_token_target         512    512         0       0        172
pumpfun/sell                     864    864         0       0        196
pumpswap/buy_exact_in           4796   4796         0       0          0
pumpswap/buy_token_target        851    851         0       0          0
pumpswap/sell                   4507   4507         0       0          0

decode+normalize us p50=11 p95=75 p99=155
quote us p50=0.295 p95=0.658 p99=0.928
unsupported reasons: no_previous_event_observed=136 previous_event_not_contiguous=232
```

Interpretation: **12,001/12,001 supported samples exact, 0 mismatch, 0 errors.**
The only non-exact bucket is `unsupported` (368 = 172 + 196), fully classified
as missing/uncorroborated pre-state. This independently reproduces the report's
central M3 claim on fresh live data.

---

## 10. Blueprint Compliance

| Invariant | Assessment | Evidence |
|---|---|---|
| **1. Neurone is parallel** | **Compliant** | shard-per-task, owned maps, no global lock; barrier test; per-shard activity in live runs |
| **2. Market state is continuously maintained** | **Compliant** | incremental `apply_*`; no per-event reconstruction; bounded volume ring |
| **3. Trading decisions are deterministic** | **Compliant** | pure decoders, exact integer math, deterministic routing/ordering, equal-input→equal-state tests |
| **7. Unknown execution conditions are unsafe** | **Compliant** | `quote()` refuses `Unknown`/`Stale`/`Invalidated`; uncorroborated pre-state → `UnsupportedState`; sweep → invalidate |
| **8. Hot path does not depend on slow external services** | **Compliant** | no RPC/HTTP/DB/FS in the hot path; only startup config read; RPC only in offline research |
| 9. Telemetry does not block execution | Compliant | atomic counters + fixed buckets; separate reporter task |
| 4–6, 10 | Out of M1–M3 scope | not penalized |

Minor caveat on invariant 7: the `spot_price_raw`/`executable_price` primitives
are ungated (L3); the *quoting* entry point is gated, which is the safety
boundary the blueprint cares about.

---

## 11. Current Readiness

### `READY FOR M4 WITH LIMITATIONS`

The M1–M3 foundation is real, tested, and independently reproduced live. The
blockers below are **not** architectural; they are specific, bounded items that
M4 (arming/quoting from state) must resolve or consciously accept.

Smallest concrete blockers, in priority order:

1. **Make invalidation recoverable (H1).** Either subscribe bonding-curve
   accounts by default, or clear `invalidated_at_slot` on a fresh authoritative
   swap/create event with `slot >= invalidated_at_slot` (not just account
   updates). Without this, swept pump.fun markets are permanently unquotable in
   the shipped runtime.
2. **Filter failed transactions (M1).** Gate decoded `swaps`/`creates`/
   invalidation on `TransactionUpdate.success`.
3. **Move the pre-state corroboration gate into the pipeline (M2).** The
   runtime needs the "prove the pre-state" logic that today exists only in
   `validate.rs`, before it can quote pump.fun SELL/token-target safely.
4. **Wire the quote engine to a consumer (I1).** Nothing in the running system
   calls `quote()` yet; M4 arming will be its first production consumer.

None of these require redesigning the parallel architecture; all are contained
changes consistent with the blueprint.

---

## 12. Recommended Next Action

**Minimum next action:** resolve blocker #1 — make M3.3 invalidation recoverable
under the default runtime configuration — because it is the one item that makes
a shipped, claimed capability (mutation-aware state that "resumes normally")
degrade to a permanent latch in practice. Concretely: decide and implement how a
fresh *authoritative* pump.fun state (either the subscribed bonding-curve
account, or a post-sweep `TradeEvent` at `slot >= invalidated_at_slot`) clears
the invalidation, with a deterministic test that exercises the **runtime**
configuration (no account subscription) rather than injecting account events.

Blocker #2 (filter `success`) is a one-line guard and can ride along. Blockers
#3 and #4 are M4 work items and should be tracked, not fixed during M3.

Do not treat this audit as authorization for any M4 implementation; it is a
read-only assessment.
