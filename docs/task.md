# NEURONE — MILESTONE 2 TASK

**Repository:** `/home/xion/neurone`  
**Source of truth:** `NEURONE_BLUEPRINT.md`  
**Scope:** Milestone 2 only — real market-state decoding.  
**Agent:** DeepSeek via Codex  
**Development tooling:** Existing Neo Agent skills at `/home/xion/neo-agent`

## Mission

Extend the completed M1 foundation from generic Yellowstone-derived state into **real, protocol-derived market state**, while preserving the existing parallel/sharded architecture.

Target:

```text
REAL YELLOWSTONE EVENTS
        ↓
PROTOCOL / ACCOUNT DECODING
        ↓
REAL MARKET STATE
   ├── price
   ├── executable reserves/liquidity
   ├── volume primitives
   ├── slot/time evolution
   └── protocol/market lifecycle state
```

**Do not turn this into a serial scanner.**

---

## 1. Read before coding

Before modifying code:

1. Read `NEURONE_BLUEPRINT.md` completely.
2. Read:
   - `docs/MILESTONE_1_REPORT.md`
   - `docs/SOLAMI_RESEARCH.md`
   - `README.md`
   - `config/default.toml`
   - relevant `src/` and `tests/`
3. Inspect the existing Neo Agent skills at `/home/xion/neo-agent` and use the relevant ones.
4. Inspect git status/history and understand the existing M1 implementation.
5. Preserve the M1 architecture unless a concrete correctness issue requires change.
6. Do not modify the blueprint to fit implementation convenience.

---

## 2. First: prove M1 live with the new credential

The operator has added the Solami credentials to the project-root `.env`.

Before new decoding work:

1. Load the environment securely.
2. Run the existing M1 runtime against the real Solami Yellowstone endpoint.
3. Verify authentication succeeds.
4. Verify real `SubscribeUpdate` events are received.
5. Verify they pass through:
   `ingest → normalize → shard → market state`.
6. Record useful live observations:
   - event types;
   - approximate event rate;
   - slot progression;
   - subscription behavior;
   - unexpected event shapes;
   - reconnect behavior if encountered.
7. Never print, commit, or persist credentials.
8. If authentication fails, stop and report the exact failure. Do not claim live verification.

M1 previously had only one unverified item: live authenticated event reception.

---

## 3. Solami documentation authority

Use `docs/SOLAMI_RESEARCH.md` as the starting research record.

If more documentation is needed, fetch the current official AI-agent index first:

`https://solami.dev/llms.txt`

Then follow the relevant official Solami documentation.

Do not rely on remembered Yellowstone APIs or stale examples. Verify protocol/API assumptions against current Solami docs and published Rust/protobuf definitions.

M1 established the current Yellowstone path:

- `https://grpc.solami.dev:443`
- `x-token` authentication
- `geyser.Geyser/Subscribe`
- `yellowstone-grpc-client`
- `yellowstone-grpc-proto`
- `processed` default commitment
- `from_slot` replay
- scoped account/transaction filters

Preserve these unless current documentation proves otherwise.

---

## 4. Pump.fun bonding-curve decoding

Implement deterministic decoding of the relevant Yellowstone account/instruction data needed to derive:

- market/pool identity;
- token mint;
- quote/base relationship where derivable;
- virtual/actual reserves exposed by the protocol;
- executable price primitive;
- liquidity / available reserves;
- relevant market lifecycle/state;
- slot/timestamp;
- volume-related information that can be reconstructed correctly.

Do not invent fields. If a field cannot yet be calculated reliably, document that limitation.

Use the current protocol layout/discriminators from authoritative sources, not guesses.

---

## 5. One additional AMM

After pump.fun decoding is correct and tested, implement **one additional AMM**.

Choose it based on:

1. relevance to Solana meme markets;
2. reliable current account/instruction layout;
3. compatibility with Yellowstone data;
4. minimum implementation complexity.

Document the selection.

Do not implement multiple AMMs merely for breadth.

---

## 6. MarketState contract

Extend the existing `MarketState`; do not create a competing market model.

Where genuinely derivable, support:

```text
market_id
token_mint
quote_mint
venue / protocol
slot
observed_at
price
base_reserve
quote_reserve
liquidity
per-slot volume primitive
market_status
protocol-specific state required later for execution
```

The exact fields may vary by venue.

Use explicit types and units. Do not silently mix raw amounts, UI amounts, lamports, SOL, or USD values.

---

## 7. Executable price

Distinguish, where the protocol allows:

```text
reference/spot price
```

from:

```text
executable buy/sell price
```

Do not treat a reserve ratio as the final executable transaction price when curve mechanics, fees, or price impact materially change it.

M2 only builds the correct primitives. Do not construct or send trades.

---

## 8. Volume model

Do **not** implement the final strategy threshold yet.

Establish the correct primitive from which later windows can be calculated, for example:

```text
per-slot buy volume
per-slot sell volume
per-slot total volume
```

or the protocol-correct equivalent.

Determine exactly what the Yellowstone stream supports. Do not fabricate historical volume.

If rolling aggregation is required, keep it per-market and compatible with thousands of concurrent market states.

The later strategy may require 5-minute volume and accelerating volume, but those qualification rules belong to M3.

---

## 9. Preserve true parallelism

Keep:

```text
Yellowstone
    ↓
Normalizer
    ↓
hash(market)
    ↓
parallel shards
    ├── market A
    ├── market B
    ├── market C
    └── ...
```

Not:

```text
receive A → decode/analyze A → receive B → decode/analyze B → ...
```

Preserve:

- deterministic shard routing;
- per-market ordering;
- no global market-state lock;
- bounded channels/backpressure;
- replay protection;
- graceful shutdown;
- existing telemetry.

If decoding is expensive, do not introduce a central serial decoding bottleneck.

---

## 10. Real-event validation + deterministic fixtures

Validate the decoders against real Yellowstone data after implementation.

Create sanitized/reproducible fixtures from real protocol data where useful, but:

- never store credentials;
- do not make normal tests network-dependent;
- keep the deterministic suite runnable without Solami access.

Add tests for:

### Protocol decoding

- valid pump.fun account;
- malformed/truncated data;
- wrong discriminator/program;
- invalid public keys;
- impossible reserve/state combinations;
- known expected reserve values;
- known expected price calculation.

### Market state

- creation/update;
- stale update rejection;
- write-version ordering;
- deterministic state transitions;
- venue separation;
- volume accumulation;
- slot progression.

### Parallelism

- multiple independent markets update concurrently;
- same market remains ordered;
- different markets do not unnecessarily block;
- deterministic shard routing;
- existing 512-market test remains green.

### Live integration

Keep live validation separate from ordinary deterministic tests.

---

## 11. Performance

Benchmark against M1 and measure:

- decode latency;
- state-update latency;
- event throughput;
- memory per active market;
- shard balance;
- rolling-volume update cost.

Report at minimum:

```text
events/sec
decode p50/p95/p99
state-update p50/p95/p99
end-to-end p50/p95/p99
```

Use realistic event patterns, not only flat synthetic bursts.

Keep the hot path small.

---

## 12. Observability

Extend telemetry only where useful:

```text
events received
events decoded
events rejected
decode failures by reason
markets created
markets updated
volume updates
stale/duplicate events
live slot
decode latency
state-update latency
```

No credential logging or uncontrolled high-cardinality hot-path logs.

---

## 13. Failure handling

Safely handle:

- malformed account data;
- unknown protocols/accounts;
- unknown discriminators;
- incomplete instruction data;
- stale events;
- duplicate events;
- stream reconnects;
- protocol decode errors;
- unexpected account layouts.

Malformed market data must not crash the runtime.

Unknown/unrecognized data should be deterministically rejected/ignored and counted.

---

## 14. Explicit non-goals

Do **not** implement:

- final market filters;
- `$10k / 5m` threshold;
- low-MC qualification;
- accelerating-volume qualification;
- safety qualification;
- pre-arming;
- capital arbitration;
- buy/sell decisions;
- TP/SL;
- wallet signing;
- transaction construction;
- Beam;
- frontend;
- LLM/narrative analysis;
- autonomous strategy modification.

M2 produces the correct market-state primitives for M3.

Do not add Blur, ShredDirect, unnecessary RPC polling, unnecessary database infrastructure, or other Solami products unless genuinely required for M2.

---

## 15. Documentation

Create/update:

```text
docs/MILESTONE_2_REPORT.md
```

Report:

1. implementation summary;
2. changed M1 files;
3. Solami documentation consulted;
4. decoded protocols;
5. exact account/instruction layouts;
6. exact price/liquidity/volume formulas;
7. units and decimal handling;
8. live-event validation;
9. tests/pass counts;
10. performance;
11. limitations/assumptions;
12. recommended M3.

Do not modify `NEURONE_BLUEPRINT.md`.

---

## 16. Verification and git

Before completion:

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets
cargo build
```

Run relevant benchmarks.

Review the diff for:

- accidental secrets;
- unnecessary files;
- blueprint changes;
- unrelated architecture changes;
- debug logging;
- dead code.

Commit M2 with a clear commit message.

**Do not begin M3 automatically.**

---

## 17. Final report

Return exactly this structure:

```text
M2 status:
Live Yellowstone:
Pump.fun decoding:
Second AMM:
MarketState:
Volume model:
Parallelism:
Tests:
Clippy:
Build:
Benchmarks:
Real-event validation:
Known limitations:
Git commit:
Recommended M3:
```

Clearly distinguish:

- verified live behavior;
- deterministic fixture/test evidence;
- unverified assumptions.

Never claim live verification unless real authenticated Yellowstone events were actually received and decoded.

---

# M2 DEFINITION OF DONE

- [ ] Real Solami authentication succeeds using the supplied environment credential.
- [ ] Real Yellowstone events are received.
- [ ] Pump.fun market/account state is decoded correctly.
- [ ] One additional AMM is decoded correctly.
- [ ] `MarketState` contains correct protocol-derived price/liquidity/reserve primitives.
- [ ] A deterministic volume primitive exists for later rolling windows.
- [ ] Existing parallel sharded architecture is preserved.
- [ ] Real-event validation succeeds.
- [ ] Deterministic decoder fixtures/tests exist.
- [ ] `cargo test` passes.
- [ ] `cargo clippy --all-targets` is clean.
- [ ] `cargo build` succeeds.
- [ ] M2 performance is measured.
- [ ] No trading/Beam/strategy logic is introduced.
- [ ] `docs/MILESTONE_2_REPORT.md` is complete.
- [ ] M2 is committed.
- [ ] Work stops.

**Do not proceed to Milestone 3 without operator authorization.**
