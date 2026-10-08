# Neurone — PumpSwap `postTokenBalances` Late-Market Bootstrap

Scope: let a PumpSwap market discovered after launch establish its current
reserves from the already-subscribed transaction stream, without waiting for a
decoded swap and without RPC / polling / backfill / token-account firehose.
Yellowstone-only, bounded, fail-closed.

Status: **IMPLEMENTED AND VALIDATED (incl. live).**

---

## 1. Problem

A PumpSwap `Pool` account stores identity, base/quote mints, vault addresses and
signed `virtual_quote_reserves:i128` — but **not** the raw vault token balances.
The runtime obtained reserves only from a decoded `BuyEvent`/`SellEvent`, so a
late-observed pool stayed `Unknown` until its next decoded swap.

Effective reserves (task-defined, matches on-chain behavior):

```
base_reserve  = raw base-vault token balance
quote_reserve = raw quote-vault token balance + virtual_quote_reserves (i128)
```

The raw balances are authoritative in `TransactionStatusMeta::post_token_balances`,
which Yellowstone already delivers on the subscribed transaction stream. That is
the bounded source used here.

## 2. Exact code changes

### `src/events.rs`

* New bounded type `VaultBalance { account, authority, mint, amount }`.
* `TransactionUpdate` gains `vault_balances: Vec<VaultBalance>`.
* New `extract_vault_balances(info)` (called from `normalize_transaction`,
  bounded: `MAX_VAULT_BALANCES = 64`, `MAX_INDEXED_KEYS = 256`). It resolves each
  `TokenBalance::account_index` through the transaction's ordered account keys
  (static, then loaded writable, then loaded read-only), parses `mint`, `owner`
  (authority) and the raw `ui_token_amount.amount`, and drops anything it cannot
  resolve. Nothing is fetched; it is pure decoding of already-delivered data.

### `src/market.rs`

* New `MarketState::apply_vault_balances(balances, slot, signature, now_ns) ->
  bool` (see §4).

### `src/shard.rs`

* In the successful-transaction arm, after swaps: for each touched market **not**
  covered by a decoded swap in the same transaction, call
  `apply_vault_balances`; on success bump `vault_balance_updates` (and
  `vault_balance_bootstraps` when the market was previously `Unknown`).

### `src/config.rs`

* New `AccountFilterConfig { programs, memcmp_base58 }` and
  `FilterConfig::extra_accounts: Vec<AccountFilterConfig>`.
* Default `extra_accounts` = `[{ programs: [pump_amm], memcmp_base58:
  base58(POOL_DISC) = "hQrXeCntzbV" }]`. `validate()` eagerly checks the extra
  program pubkeys. (Default `transaction_programs` now clones `pumpswap` for
  reuse.)

### `src/ingest/solami.rs`

* `build_subscribe_request` now emits the primary account filter **and** one
  bounded filter per `extra_accounts` entry (named `accounts_extra_{i}`), via a
  small `memcmp_disc_filter` helper. New test
  `default_subscription_targets_pump_swap_pools`.

### `src/telemetry.rs`

* New counters `vault_balance_updates` and `vault_balance_bootstraps` (+ report
  line) so the new path is observable live.

### `config/default.toml`, `config/live_probe.toml`

* `default.toml` documents/adds the `[[ingest.filters.extra_accounts]]` Pool
  filter; `live_probe.toml` sets `extra_accounts = []` to stay narrow.

### Tests

* `tests/pumpswap_bootstrap.rs` (new, 12 tests); field additions
  (`vault_balances: Vec::new()`) in the other `TransactionUpdate` literals;
  `tests/live_solami.rs` now also clears `extra_accounts` to keep its stream
  narrow (it clears the other account filters already).

## 3. Subscription / filter changes

Two bounded account filters on the single subscription (unchanged architecture):

| Filter | owner | offset-0 memcmp | delivers |
|---|---|---|---|
| `accounts` | pump.fun | `4y6pru6YvC7` (BondingCurve) | bonding curves |
| `accounts_extra_0` | pump_amm | `hQrXeCntzbV` (Pool) | PumpSwap `Pool` accounts |

Ownership of the Pool filter is one account type (bounded) — **no** SPL
token-account subscription, no firehose, no dynamic resubscription. The first
`Pool` account update creates the pool market with metadata but `Unknown`
reserves. (Verified by `neurone check`: `filters(accounts=2 …)`.)

## 4. Pool/vault association and state transition

`apply_vault_balances` (in `MarketState`, where the expected values live):

1. Require the pool's metadata: `pool_base_token_account`, `pool_quote_token_account`,
   `base_mint`, `quote_mint` (set by the `Pool` account decode). Otherwise → no-op.
2. Match balances by **exact vault pubkey AND expected mint AND authority == pool**:
   * base  ↔ `pool_base_token_account` with `mint == base_mint`
   * quote ↔ `pool_quote_token_account` with `mint == quote_mint`
   A balance from any other account/pool cannot match.
3. Require **both** present, else → no-op (stays `Unknown`).
4. `effective_quote = quote_raw as i128 + virtual_quote_reserves` (signed, exact
   integer arithmetic); refuse if negative or overflowing.
5. Refuse if it would regress newer state (`reserves_known && slot <
   last_reserve_slot`).
6. Establish: `base_reserve = base_raw`, `quote_reserve = effective_quote`,
   `reserves_known = true`, `last_reserve_slot = slot`,
   `last_reserve_signature = tx`, `last_reserve_timestamp = None`.

Ordering: a decoded swap event in the same transaction is authoritative and the
vault path is skipped for that market; the existing `Stale`/`Invalidated`
fail-closed semantics are untouched (the new path only sets reserves; a genuine
non-trade mutation still invalidates). A pool with only one vault, wrong
pubkey/mint, wrong authority, missing metadata, or a negative effective quote
stays `Unknown`. No reserve is ever fabricated.

## 5. Tests

`tests/pumpswap_bootstrap.rs` (12, deterministic, real engine/shards):

1. `pool_metadata_decodes_from_yellowstone_account_data` — real Pool bytes
   through `normalize` → metadata incl. signed i128, `Unknown`.
2. `late_pool_bootstraps_from_post_token_balances_without_swap` — no create, no
   predecessor; vault balances (no swap) → `Known`; `quote = raw + virtual`.
3. `base_vault_must_match_pubkey_and_mint`.
4. `quote_vault_must_match_pubkey_and_mint`.
5. `unrelated_balances_cannot_contaminate_pool` (wrong authority).
6. `missing_one_vault_balance_keeps_unknown`.
7. `missing_pool_metadata_keeps_unknown` (no fabrication).
8. `signed_negative_virtual_quote_reserves_is_handled` (700 accepted;
   negative effective refused).
9. `existing_pumpswap_swap_path_still_works`.
10. `swap_event_is_authoritative_over_vault_balances_in_same_tx`.
11. `stale_and_invalidated_semantics_unchanged`.
12/13. `normalizer_extracts_post_token_balances` (index→pubkey, mint,
   authority, raw amount).

`src/ingest/solami.rs`: `default_subscription_targets_pump_swap_pools` (bounded
Pool filter; exactly two account filters; no token-program owner).

## 6. Validation

| Command | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --all-targets --all-features -- -D warnings` | clean (0 warnings) |
| `cargo test --all-targets` | **126 passed, 0 failed, 1 ignored** |
| `cargo build --release --locked` | ok |
| `cargo test --test live_solami -- --ignored` | 1 passed (3/3 consecutive runs) |
| `NEURONE_VALIDATE_SECONDS=20 neurone validate` | M3 parity unchanged: 4,233 / 4,233 supported exact, **0 mismatch, 0 errors** |
| `neurone check` | `filters(accounts=2 txs=1 …)` — bounded (curves + pools) |

## 7. Live result

Two live `neurone run` sessions (default config, Solami):

* ~70 s: `vault_balance_updates=4476`, `swaps_pumpswap=114018`, 4,792 markets.
* ~45 s: `vault_balance_updates=4202`, `vault_balance_bootstraps=4`,
  `swaps_pumpswap=65961`, 3,706 markets.

`vault_balance_bootstraps` counts markets taken `Unknown → Known` **by the new
`postTokenBalances` path** (i.e. no decoded swap in that transaction). It was
**4** in the sampled session: real PumpSwap pools bootstrapped their current
reserves without a prior decoded swap. This is the required live proof.

## 8. Performance / overhead impact

* Extraction is O(#store balances), bounded to 64 entries and 256 indexed keys
  per transaction; no allocation beyond the bounded `Vec`, no I/O.
* Application is O(touched keys × balances) with a tiny bound; only for
  successful transactions.
* One extra bounded account filter (PumpSwap `Pool` accounts) is added to the
  account stream — a single account type, not a token-account firehose.
* No RPC, no polling, no backfill, no dynamic subscription, no shard redesign.

## 9. Remaining limitations

1. The path only fires when a `pump_amm` transaction touching the pool is
   observed (a swap, deposit, withdrawal, fee op, …). A pool with *no* observed
   activity since bootstrap remains `Unknown` — inherent and fail-closed; it
   does **not** require launch history or a predecessor chain.
2. Old/short `Pool` accounts that cannot decode `virtual_quote_reserves`
   (`decode_account` returns `None`, e.g. <261-byte layouts) cannot bootstrap via
   this path; they still work through the existing swap-event path. Fail-closed.
3. The decoded swap event remains authoritative for its own transaction; the
   vault path deliberately defers to it (no double source of truth).
4. `previous_event_not_contiguous` root cause is untouched (out of scope).

## 10. Verdict

`IMPLEMENTED — PumpSwap late markets bootstrap from bounded transaction postTokenBalances.`

Yellowstone-only, bounded, no token-account firehose, no RPC/polling/backfill;
`Unknown` until both authoritative vault balances and Pool metadata are present
and consistent; existing PumpSwap swap-event and Pump.fun behavior and the
stale/invalidated fail-closed semantics are unchanged. Live runs show real pools
transitioning `Unknown → Known` via the new path.
