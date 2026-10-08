# Neurone — PumpSwap Late-Market Bootstrap

Scope: try to let a late-observed PumpSwap pool bootstrap its current reserves
from streamed Yellowstone data (so it need not wait for its next decoded swap).
Yellowstone/gRPC only; no RPC, no polling, no historical backfill, no M4.

**Outcome: STOPPED.** The authoritative data required (the pool's raw vault
token balances) cannot be delivered to Neurone by any *bounded, static*
Yellowstone account subscription, and the only ways to deliver it (a Token-program
account firehose, or unbounded/dynamic per-vault filters) are explicitly out of
scope. No production code was changed.

---

## 1. Current limitation

PumpSwap `Pool` accounts (`pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA`, disc
`f19a6d0411b16dbc`) do **not** store raw base/quote reserves. The current Pool
layout (verified against the current official `idl/pump_amm.json`) is:

```
pool_bump:u8, index:u16, creator, base_mint, quote_mint, lp_mint,
pool_base_token_account, pool_quote_token_account, lp_supply:u64,
coin_creator, is_mayhem_mode:bool, is_cashback_coin:bool,
virtual_quote_reserves:i128, creator_fee_bps:u64, can_edit_creator_fee:bool,
is_holder_reward:bool, protocol_fees:u64, creator_fees:u64
```

Effective reserves (task-defined, matches on-chain behavior):

```
effective_quote_reserves = raw quote-vault token balance + virtual_quote_reserves
base_reserves            = raw base-vault token balance
```

The raw vault balances live in the two SPL token accounts named by the Pool
(`pool_base_token_account`, `pool_quote_token_account`), which the Pool account
does not restate.

Today the runtime gets PumpSwap reserves only from a decoded `BuyEvent` /
`SellEvent` (which carry `pool_base_reserves` / `pool_quote_reserves`). So a pool
observed *after* launch stays `ReserveState::Unknown` (non-tradable, fail-closed)
until its next decoded swap. That is the limitation. (Note: unlike pump.fun
bonding curves, PumpSwap `Pool` accounts are not even in the default account
subscription today — `account_programs = [pump.fun]` — so pools are currently
discovered via the swap transaction stream.)

---

## 2. Exact Yellowstone data required

To bootstrap without a swap, Neurone must observe, from the live stream:

1. the PumpSwap `Pool` account (identity, `pool_base_token_account`,
   `pool_quote_token_account`, `base_mint`, `quote_mint`,
   `virtual_quote_reserves:i128`); and
2. the two vault **token-account** balances (`amount:u64` at offset 64 of the SPL
   token account), together with the vault's `mint` (offset 0) and authority
   (offset 32) for positive association.

(1) is obtainable from a bounded static filter. (2) is the blocker.

---

## 3. Investigation findings

### 3.1 Account subscription/filter architecture

`src/ingest/solami.rs::build_subscribe_request` builds **one** account filter
(`SubscribeRequestFilterAccounts { account, owner, filters }`). The Yellowstone
proto (`geyser.proto`) allows exactly these narrowing primitives:

```
account:  repeated string   // explicit pubkeys (OR)
owner:    repeated string   // owning *programs* (OR)
filters:  memcmp(offset,data) | datasize(n) | token_account_state(bool) | lamports(cmp)
          (multiple filters are AND-ed within one filter)
```

Filters are static: the request is built once per connection and is not modified
mid-stream (only pings are sent). Multiple *named* filters OR together, but a
single filter cannot express "one of N authority values".

### 3.2 Association (feasible, and verified on-chain)

A PumpSwap pool can be associated with its vaults using only Yellowstone data:
the Pool account names the two vault pubkeys, and each vault's SPL-token
`authority` field (offset 32) equals the **pool** address (verified for
240/240 vaults across 120 pools). The vaults are owned by the SPL Token program
(`Tokenkeg…` 202, `Tokenz…` 38). `DecodedAccount` already carries
`pool_base_token_account` / `pool_quote_token_account` and the decoder already
reads `virtual_quote_reserves:i128`.

### 3.3 The blocker: vault accounts are pool-specific, so no bounded filter selects them

Across 120 sampled pools (240 vault accounts), the vault `authority` is the pool
in **240/240** cases and there are **120 distinct authorities** (one per pool):
there is no shared/global authority. The vault *owning program* is the SPL Token
program, not `pump_amm`. Consequently, to receive a pool's vault updates through
the account subscription you would need a filter per pool:

```
owner = [Tokenkeg…, Tokenz…]
filters = [ memcmp(offset = 32, data = <pool_authority>) ]
```

That is one filter per pool, with the pool set discovered dynamically — i.e.
unbounded and time-varying. A single filter cannot match "authority ∈ {pool₁,
pool₂, …}". The remaining static options are firehoses:

- `owner = [Tokenkeg…, Tokenz…]` (all SPL token accounts on the network), or
- `owner = [Tokenkeg…, Tokenz…] + token_account_state = true` (same set, boundedly
  filtered only by account *type*, not by pool).

Both stream the entire token-account universe — exactly the firehose the
architecture forbids (cf. `config/default.toml`'s firehose guard on the
transaction filter and the blueprint's bounded/no-firehose requirement).

Evidence: `research/pumpswap_bootstrap/probe.py` (read-only).

---

## 4. Why this cannot be implemented under the scope lock

The task requires the vault data to reach Neurone **without RPC, polling,
historical backfill, and without redesigning ingestion**, and the architecture
must remain bounded. The available ways to deliver `pump_amm` vault token-account
updates are:

| Option | Bounded? | Yellowstond-only? | Verdict |
|---|---|---|---|
| Static filter `owner=Tokenkeg/Tokenz` (all token accounts) | No — firehose | yes | violates bounded/no-firehose design |
| Static filter + `token_account_state` | No — firehose | yes | same |
| One static `memcmp(offset=32, authority)` filter **per pool** | No — unbounded, must grow with the pool set | yes | unbounded; the pool set is open-ended |
| **Dynamic** resubscription to add per-vault/per-pool filters | No — unbounded; adds a live subscription-mutation path | yes | explicitly excluded: "do not redesign Yellowstone ingestion" |
| RPC `getTokenAccountBalance` / `getMultipleAccounts` at pool discovery | n/a | **no** | explicitly excluded |

None is permissible. Therefore, per the task's instruction — *"If the necessary
authoritative Yellowstone data cannot be obtained safely with the existing
architecture, STOP and report why rather than inventing a fallback"* — this task
STOPs here.

### 4.1 Alternative explicitly considered and rejected: transaction `postTokenBalances`

Yellowstone transaction updates carry `TransactionStatusMeta::post_token_balances`
(account index, mint, owner, amount) for accounts written by the transaction, and
the existing transaction stream already includes `pump_amm`. That data is
authoritative and bounded, and could (in principle) establish vault balances when
a `pump_amm` transaction touching a pool is observed.

It is **not** a substitute for the requested mechanism, so it was not
implemented:

* It is not *account state*; it does not deliver the "token-account updates"
  (and their out-of-order / same-slot / repeated-update semantics) the task and
  its tests describe.
* It only fires when a `pump_amm` transaction touching the pool is observed. For
  the overwhelmingly common case that is a **swap**, whose decoded event already
  establishes reserves via the existing path — so it adds nothing there. It would
  help only for non-swap activity (pool creation, deposits, withdrawals).
* It would require additionally subscribing the `pump_amm` `Pool` accounts (to
  obtain `virtual_quote_reserves` and the vault→pool mapping), which is a
  behavior change of its own.

Treating it as a drop-in would be exactly the "inventing a fallback" the task
prohibits; it is recorded here as a candidate for a separately-authorized,
separately-scoped change.

---

## 5. Exact implementation

**None.** No production file was modified. The runtime keeps the existing,
safe behavior:

```
late PumpSwap pool → discovered via a decoded swap event → reserves Known
                   → (no reserve observations before that) → Unknown / non-tradable
```

## 6. Subscription changes

**None.** The default subscription is unchanged (`account_programs = [pump.fun]`
with the bonding-curve memcmp; transactions for `pump.fun` + `pump_amm`). No new
account filter was added, because every candidate is either a firehose or
unbounded/dynamic.

## 7. State-association logic (design that would be used if delivery existed)

Recorded for completeness, since it is the part that *is* feasible:

* On a `Pool` account update: set `venue = PumpSwap`, record
  `base_mint`, `quote_mint`, `pool_base_token_account`,
  `pool_quote_token_account`, decode `virtual_quote_reserves:i128` (signed),
  and remember the pool's own address as the expected vault authority.
* On a token-account update whose pubkey equals the pool's `*_token_account`
  **and** whose SPL `authority` (offset 32) equals the pool address: record the
  raw `amount` for the side whose `mint` matches the pool's base/quote mint.
  A vault from another pool, or with a mismatched authority/mint, is ignored.
* Only when **both** raw balances and the Pool metadata are present:
  `base_reserve = raw_base`, `quote_reserve = raw_quote + virtual_quote_reserves`
  (checked/saturating on the signed sum), `reserves_known = true` → `Known`.
  Until then the market stays `Unknown` (no fabrication).
* Slot/`write_version`/invalidation rules are unchanged, so this composes with
  the existing `Stale`/`Invalidated` fail-closed semantics.

This logic is straightforward; it is the *delivery* of the token-account updates
that is infeasible.

## 8. Tests

None added: there is no implemented behavior to pin, and the required tests
(late pool boots from vault account updates, out-of-order vault delivery, etc.)
presuppose a subscription that does not exist and cannot be added within scope.

## 9. Validation

The tree is unchanged, and the existing suite is green:

| Command | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --all-targets --all-features -- -D warnings` | clean (0 warnings) |
| `cargo test --all-targets` | **113 passed, 0 failed, 1 ignored** |
| `cargo build --release --locked` | ok |

(No live-validation change was needed; the PumpSwap swap state path is
unchanged.)

## 10. Performance / overhead impact

None — no code changed. For the record, the rejected firehose option would stream
the entire SPL token-account universe (orders of magnitude above the current
bonding-curve feed) and is incompatible with the bounded hot path; the rejected
dynamic-filter option would grow the subscription set without bound as pools are
discovered.

## 11. Remaining limitations

1. A late-observed PumpSwap pool still stays `Unknown` until its first decoded
   swap — the existing, safe, fail-closed behavior. Because active pools trade
   frequently, this is a short window in practice; it is unbounded only for
   pools that never trade again.
2. PumpSwap `Pool` accounts are not currently account-subscribed; pools are
   discovered through the swap transaction stream. (This is separate from the
   vault-data blocker and does not by itself enable a bootstrap.)
3. The `previous_event_not_contiguous` root cause is untouched (out of scope).

## 12. Verdict

`STOPPED — PumpSwap vault token-account state is not obtainable with the existing bounded/static Yellowstone subscription architecture.`

The pool↔vault association and the effective-reserve composition are fully
understood and implementable, but the vault token-account updates cannot be
delivered to Neurone without either an SPL-token-program firehose, an unbounded
dynamic per-pool/per-vault subscription, or RPC — all excluded by this task's
constraints. No production change was made and the existing fail-closed behavior
is retained. A separately-authorized follow-up could pursue the bounded
transaction-`postTokenBalances` route (§4.1), which is Yellowstond-only and
bounded but is a different mechanism than the one specified here.
