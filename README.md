# Neurone

Neurone is a standalone, parallel, event-driven Solana market-state runtime.
It is deliberately **not** inherited from Xion and contains **no trading logic
in this milestone**.

This repository implements **Milestones 1 and 2**:

```text
Solami Yellowstone gRPC
        │
        ▼
Rust gRPC ingestion  (auth, keepalive, reconnect, from_slot replay)
        │
        ▼
event normalization + protocol decoding
   ├── pump.fun bonding curve  (BondingCurve account, TradeEvent, CreateEvent)
   └── pump.swap AMM           (Pool account, BuyEvent/SellEvent)
        │
        ▼
parallel sharded market state  (hash(market) -> shard -> reserves, price, volume)
        │
        ▼
lock-free telemetry  (counters + latency histograms)
```

Milestone 1 built the parallel runtime and proved live authenticated Solami
reception. Milestone 2 added real, protocol-derived market state (price,
reserves, liquidity, per-slot volume) while keeping the same sharded
architecture.

The source of truth is [`NEURONE_BLUEPRINT.md`](./NEURONE_BLUEPRINT.md);
the milestone definition and stop condition are in [`task.md`](./task.md).
The record of the Solami interface actually used is in
[`docs/SOLAMI_RESEARCH.md`](./docs/SOLAMI_RESEARCH.md), and the milestone
completion report is in [`docs/MILESTONE_1_REPORT.md`](./docs/MILESTONE_1_REPORT.md).

## Design invariants honoured here

* **Parallel.** Each shard is an independent task owning its markets. There is
  no global lock over market state. Independent markets advance concurrently.
* **Continuous.** Market state is mutated incrementally; it is never rebuilt
  from scratch per event.
* **Deterministic.** `hash(pubkey) -> shard` uses a fixed FNV-1a hash (never
  `std`'s randomised hasher). The same events in the same order produce the
  same state.
* **Minimal hot path.** Normalization allocates once; telemetry is atomics
  only; no I/O in the processing path.
* **Fail closed.** Unknown/ignored updates are counted, not guessed at.

## Quick start

```bash
cargo build
cargo test            # unit + architecture + reconnect tests
cargo run -- check    # validate config + subscription shape (no socket)
```

Offline benchmark (no credential required):

```bash
# peak throughput (producer runs flat out)
NEURONE_BENCH_EVENTS=100000 cargo run -- bench

# steady-state latency (producer paced to 10k events/s)
NEURONE_BENCH_EVENTS=20000 NEURONE_BENCH_RATE=10000 cargo run -- bench

# pure protocol-decode throughput over the real fixtures
NEURONE_DECODE_ITERS=1000000 cargo run -- bench
```

Live integration test (requires a credential; separated from the deterministic
suite and ignored by default):

```bash
SOLAMI_GRPC_TOKEN=... cargo test --test live_solami -- --ignored --nocapture
```

Live run against Solami Yellowstone:

```bash
cp .env.example .env
# put your key in SOLAMI_GRPC_TOKEN (or export it)
cargo run -- run
```

If the credential is missing or invalid the process fails closed: it logs the
gRPC status (`UNAUTHENTICATED: invalid api key`), backs off exponentially and
reconnects. It never routes events when the stream is not authenticated.

## Configuration

Precedence: environment > TOML file > compiled defaults.

| Variable | Meaning |
| --- | --- |
| `SOLAMI_GRPC_TOKEN` (or `SOLAMI_API_KEY`) | gRPC `x-token` credential (secret, env only) |
| `SOLAMI_GRPC_ENDPOINT` | override endpoint (default `https://grpc.solami.dev:443`) |
| `NEURONE_CONFIG` | path to TOML config (default `config/default.toml`) |
| `NEURONE_SOURCE` | `solami` or `simulated` |
| `NEURONE_SHARDS` | number of shards |
| `NEURONE_WORKER_THREADS` | tokio worker threads |
| `RUST_LOG` | tracing filter |

Secrets are **never** read from the TOML file and `.env` is gitignored.

## Commands

| Command | Effect |
| --- | --- |
| `neurone run` | Run the configured ingestion source + sharded engine + telemetry |
| `neurone check` | Validate configuration and print the resulting subscription shape |
| `neurone bench` | Drive the engine with the deterministic synthetic source and report throughput/latency |

## Module map

| Module | Responsibility |
| --- | --- |
| `config` | Typed configuration, env overrides, validation |
| `error` | Typed, stage-scoped errors |
| `shutdown` | Cooperative shutdown (watch channel) |
| `clock` | Monotonic nanosecond clock |
| `hash` | Deterministic FNV-1a shard routing |
| `events` | Normalized event types + Yellowstone normalization |
| `decode` | Protocol decoders (pump.fun bonding curve, pump.swap AMM) + Borsh + PDA |
| `market` | Incremental per-market state |
| `shard` | Independent shard task, dedup ring, shard-local maps |
| `engine` | Shard routing + snapshots |
| `ingest::solami` | Live Yellowstone gRPC ingestion (auth, ping, reconnect) |
| `ingest::simulated` | Deterministic offline source |
| `telemetry` | Lock-free counters + latency histograms + reporter |
| `runtime` | Task assembly, supervision, graceful shutdown |
| `bench` | Throughput/latency harness |

## Protocol decoding (Milestone 2)

Venues are decoded against the **official pump.fun IDLs**
(`pump-fun/pump-public-docs`), not from memory:

* pump.fun bonding curve — `BondingCurve` account, `TradeEvent`, `CreateEvent`;
  market identity is the bonding-curve PDA of the mint.
* pump.swap AMM — `Pool` account, `BuyEvent`/`SellEvent`; market identity is the
  pool account.

All reserves/amounts are **raw integer units**; prices are exact integer ratios
(`Ratio`), and a separate `executable_price` primitive applies constant-product
impact and fees. `fixtures/` holds real mainnet bytes used by
`tests/real_fixtures.rs`, so the decoders are pinned to real data without being
network-dependent.

## Not implemented (by design)

Live trading, transaction construction/signing, Beam, TP/SL, capital
arbitration, strategy scoring, LLM calls, Blur, ShredDirect, Index Engine,
webhooks, Mirage, frontend, heavyweight databases. See task section 10.
