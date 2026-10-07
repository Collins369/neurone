# Neurone

Neurone is a standalone, parallel, event-driven Solana market-state runtime.
It is deliberately **not** inherited from Xion and contains **no trading logic
in this milestone**.

This repository currently implements **Milestone 1**:

```text
Solami Yellowstone gRPC
        │
        ▼
Rust gRPC ingestion  (auth, keepalive, reconnect, from_slot replay)
        │
        ▼
event normalization  (compact owned events, no protobuf downstream)
        │
        ▼
parallel sharded market-state engine  (hash(market) -> shard -> owned state)
        │
        ▼
lock-free telemetry  (counters + latency histograms)
```

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
| `market` | Incremental per-market state |
| `shard` | Independent shard task, dedup ring, shard-local maps |
| `engine` | Shard routing + snapshots |
| `ingest::solami` | Live Yellowstone gRPC ingestion (auth, ping, reconnect) |
| `ingest::simulated` | Deterministic offline source |
| `telemetry` | Lock-free counters + latency histograms + reporter |
| `runtime` | Task assembly, supervision, graceful shutdown |
| `bench` | Throughput/latency harness |

## Not implemented (by design)

Live trading, transaction construction/signing, Beam, TP/SL, capital
arbitration, strategy scoring, LLM calls, Blur, ShredDirect, Index Engine,
webhooks, Mirage, frontend, heavyweight databases. See task section 10.
