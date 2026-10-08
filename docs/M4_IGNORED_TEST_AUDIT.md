# Neurone — Ignored-Test Audit (M4 follow-up)

Scope: identify and critically assess the single ignored test in the repository.
No production code was modified. `max_drawdown_bps` remains unset/`None`; no
SOL/USD feed was implemented.

---

## 1. Exact ignored test

`live_authenticated_stream_decodes_markets` — integration test in
`tests/live_solami.rs` (test binary `live_solami`; the file is its own module
root), declared at `tests/live_solami.rs:31`.

Only ignored test in the repo:

```
cargo test --all-targets -- --ignored --list
  ... tests/live_solami.rs
  live_authenticated_stream_decodes_markets: test
  1 test, 0 benchmarks
  (every other target: 0 tests, 0 benchmarks)
```

## 2. What the test does

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a live Solami credential"]
async fn live_authenticated_stream_decodes_markets()
```

* Reads a credential from the environment
  (`SOLAMI_GRPC_TOKEN` / `SOLAMI_GRPC_API_KEY` / `SOLAMI_API_KEY`); if absent it
  prints `skipping: no Solami credential in environment` and returns.
* Clears the account filters to keep the stream narrow (`account_programs`,
  `account_addresses`, `extra_accounts`).
* Starts the real engine + `TonicConnector` against Solami Yellowstone, waits up
  to 30 s for live events, snapshots the shards, prints a live pump.swap quote.
* Asserts `events_received > 0`, `events_decoded > 0`,
  `active_market_states > 0`, and `with_reserves > 0`.

## 3. Why it is `#[ignore]`

The reason is on the attribute and in the module doc: it requires a live Solami
credential and is **intentionally separate from the deterministic suite**
(`git log -S'#[ignore'` shows no later change).

## 4. When/why it was originally ignored

It was already ignored in the file's first commit:

```
8656a03  Milestone 2: decode pump.fun bonding curve + pump.swap AMM into real market state
```

i.e. from inception, so the deterministic suite stays hermetic and never
requires a credential, network access, or paid/dynamic gRPC.

## 5. Is the reason still valid?

Yes. Classification: **intentional live/integration test**, **environment-
dependent**, **non-hermetic** (depends on live chain traffic). It is not
obsolete, not broken, and not incorrectly ignored:

* It needs an external credential and real network I/O; the default `cargo test`
  has neither. Even with a credential present, un-ignoring it would attach the
  normal suite to Solami's paid/dynamic gRPC.
* It is mildly **flaky by nature**: it requires some reserve-known market to
  appear within a 30 s narrow-stream window.
* It still has value: it is the only end-to-end check of the real client +
  normalizer + shards + decoder + quote path.

## 6. Result of attempting to run it

With the credential present:

```
cargo test --test live_solami --locked -- --ignored --nocapture
  live_authenticated_stream_decodes_markets ... ok
  test result: ok. 1 passed; 0 failed; 0 ignored  (4.64 s)
```

(Connected, live decode, 8 markets; `--list` shows `1 test` for this ignored
test.)

## 7. Recommended action

**Keep `#[ignore]`.** Do not remove it — that would pull live network I/O and a
paid gRPC stream into the default suite and make `cargo test` environment-
dependent/flaky. No rename is needed (the name is accurate), and no new
documentation is required: the reason is already on the attribute and in the
module doc, and the test is referenced in `docs/M1_M2_M3_CODEBASE_AUDIT.md`, the
M3.3 reports, and the M4 reports. This is a correctly-ignored, credential-gated
live test — not a hidden failure and not a count-padding artifact.

## 8. Complete test / quality results

```
cargo test --all-targets                        181 passed, 0 failed, 1 ignored
cargo test --all-targets -- --ignored --list    only live_authenticated_stream_decodes_markets
cargo test --test live_solami -- --ignored      1 passed (4.64 s)
cargo fmt --check                               clean
cargo clippy --all-targets --all-features -- -D warnings   clean (0 warnings)
```

Per-suite breakdown of `cargo test --all-targets`: lib 55 · main 0 ·
architecture 8 · decode_and_state 6 · failed_transaction 4 · ingest_reconnect 1
· late_market 5 · live_solami 0 (1 ignored) · m3_parity 12 · mutation 10 ·
pumpswap_bootstrap 12 · quote_parity 5 · real_fixtures 7 · reserve_state 6 ·
strategy 50 ⇒ 181 passed / 0 failed / 1 ignored.

## 9. Task confirmations

* No production code was modified in this task (inspection + test runs only).
* `max_drawdown_bps` remains unset/`None` (`src/strategy.rs:99`; left commented
  in `config/default.toml:111`).
* No SOL/USD feed was implemented.
