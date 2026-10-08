# Task: Yellowstone postTokenBalances PumpSwap Late-Market Bootstrap

## Objective
Implement the bounded Yellowstone-only PumpSwap late-market bootstrap identified in `PUMPSWAP_LATE_BOOTSTRAP_REPORT.md` using transaction `postTokenBalances`.

The goal is to allow a PumpSwap market discovered after launch to establish current reserves without waiting for its next decoded swap, while preserving Neurone's architecture:
- Yellowstone/gRPC only
- no RPC
- no polling
- no SPL-token account firehose
- no dynamic per-vault account subscription
- no historical backfill
- parallel market state
- fail closed when current state cannot be established

## Authoritative findings
The report establishes:
- PumpSwap Pool accounts contain the pool identity, base/quote mints, base/quote vault addresses, and signed `virtual_quote_reserves:i128`.
- Raw vault balances are not stored in the Pool account.
- Yellowstone transaction metadata already contains `postTokenBalances` for accounts written by the transaction.
- `postTokenBalances` is authoritative and bounded.
- The existing PumpSwap transaction stream is already subscribed.
- The report did NOT implement this path because it was outside the previous task scope. This task explicitly authorizes it now.

Effective reserves:
- `base_reserve = raw base-vault token balance`
- `quote_reserve = raw quote-vault token balance + virtual_quote_reserves`

## Required implementation

### 1. Inspect existing transaction metadata support
Find how Yellowstone `TransactionStatusMeta::post_token_balances` is currently exposed/normalized in the codebase.
Do not add a second parser if the protobuf data is already available.

### 2. Ensure PumpSwap Pool metadata is available
The runtime needs the Pool's:
- pool address
- base mint
- quote mint
- pool_base_token_account
- pool_quote_token_account
- virtual_quote_reserves

Add the minimum PumpSwap Pool account subscription/filter necessary to receive Pool account updates.
This is bounded: subscribe to the PumpSwap AMM program's Pool accounts using the Pool discriminator/layout, NOT all token accounts.
Do not subscribe to SPL token accounts globally.

### 3. Bootstrap from transaction postTokenBalances
When a PumpSwap transaction is observed, inspect its `postTokenBalances`.

For a known Pool:
- identify the base vault by exact pubkey match to `pool_base_token_account`;
- identify the quote vault by exact pubkey match to `pool_quote_token_account`;
- verify the token mint matches the Pool's expected base/quote mint;
- obtain the post-transaction raw `amount` for each vault;
- combine with the Pool's current `virtual_quote_reserves`.

Only establish `Known` when the required current Pool metadata and both required vault balances are available and internally consistent.

### 4. Late-discovery behavior
A Pool discovered after launch must NOT require its launch history or predecessor chain.

Desired behavior:

Pool discovered
  -> Pool metadata known
  -> next relevant PumpSwap transaction containing postTokenBalances
  -> vault balances established
  -> effective reserves established
  -> `ReserveState::Known`
  -> normal continuous Yellowstone observation

If a transaction does not contain both required vault balances, remain `Unknown`.
Do not fabricate or infer missing reserves.

### 5. Preserve existing swap path
Existing BuyEvent/SellEvent reserve updates remain authoritative and must continue to work.
Do not replace or weaken them.

If a swap event supplies reserves directly, the existing path may establish/update state as it does today.
The new postTokenBalances path is specifically for bootstrap/current-state establishment when needed.

### 6. Ordering and freshness
Respect existing slot/write-version/state ordering semantics.
Do not apply an older transaction's postTokenBalances over a newer authoritative state.
Do not let unrelated token accounts contaminate a Pool.
Do not mix balances from different transactions into a synthetic state unless the existing state model explicitly permits that and the required consistency conditions are satisfied.

A conservative implementation is preferred: if Pool metadata and the two balances cannot be associated to the same sufficiently current observation, remain `Unknown`.

### 7. Virtual quote handling
Decode/use `virtual_quote_reserves` as signed `i128`.
Compute the effective quote reserve exactly with integer arithmetic.
Do not use floating point.
Do not alter existing quote math unless a concrete incompatibility is demonstrated.

### 8. No architecture creep
Do NOT:
- add RPC
- add polling
- add historical backfill
- subscribe to all SPL token accounts
- create dynamic per-vault account subscriptions
- redesign the Yellowstone ingestion layer
- redesign sharding
- modify strategy
- modify execution
- investigate `previous_event_not_contiguous`
- start M4 strategy work

## Tests
Add deterministic tests proving:
1. PumpSwap Pool metadata can be received/decoded from Yellowstone account data.
2. A late PumpSwap market can bootstrap reserves from `postTokenBalances` without a swap event carrying reserves.
3. Base vault is matched by exact pubkey and expected mint.
4. Quote vault is matched by exact pubkey and expected mint.
5. Wrong vault/account cannot contaminate the market.
6. Missing one vault balance keeps the market `Unknown`.
7. Missing Pool metadata keeps the market `Unknown`.
8. Signed negative `virtual_quote_reserves` is handled correctly.
9. Existing PumpSwap Buy/Sell reserve updates still work.
10. Existing Pump.fun behavior remains unchanged.
11. Existing stale/invalidated fail-closed behavior remains unchanged.
12. Late market requires no launch history or predecessor chain.

## Live validation
Run:
- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-targets`
- `cargo build --release --locked`
- existing live Solami/Yellowstone validation if practical

During live validation, specifically demonstrate at least one PumpSwap pool that was not bootstrapped from a prior swap event can become `Known` through the new Yellowstone path, if such a case can be observed safely. If live proof cannot be obtained, report that honestly; do not fake it.

## Deliverable
Create:
`PUMPSWAP_POSTTOKEN_BOOTSTRAP_REPORT.md`

Report:
- exact files changed
- subscription/filter changes
- postTokenBalances parsing path
- Pool/vault association logic
- state transition semantics
- tests
- validation
- live result
- performance/overhead impact
- remaining limitations

## Final acceptance criteria
The implementation is accepted only if:
- it remains Yellowstone-only;
- it does not create a token-account firehose;
- it does not require RPC/polling;
- late PumpSwap markets can establish current reserves from bounded transaction metadata when sufficient data is present;
- missing/inconsistent data remains fail-closed;
- existing PumpSwap and Pump.fun behavior does not regress.

If the implementation cannot safely satisfy these criteria, STOP and report the exact blocker instead of inventing a fallback.
