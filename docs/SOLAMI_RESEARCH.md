# Solami Interface Research (Milestone 1)

This document records the Solami interface **actually used** by Neurone's
Yellowstone ingestion, and where each fact came from. It exists so the
implementation is not "from memory".

## How the documentation was obtained

`https://solami.dev` is a client-rendered single-page app, so the prose lives
in the JS bundle. The research path was:

1. `https://solami.dev/llms.txt` — the structured index Solami provides for AI
   agents (product list, endpoints, pricing warnings).
2. The site bundle `https://solami.dev/assets/index-Cc3yJk8O.js`, which embeds
   the full docs pages (`/docs/grpc`, `/docs/endpoints`, `/docs/errors`,
   `/docs/policies-limits`, `/docs/sdk`, `/docs/shreds`, guides) **and** the
   protobuf descriptor as a JSON string.
3. `https://crates.io/api/v1/crates/...` — to confirm the published Rust
   client crates and their exact versions/dependencies.
4. A live connectivity probe against the real endpoint (below).

## 1. Endpoint and connection

| Fact | Value |
| --- | --- |
| Endpoint | `grpc.solami.dev:443` (TLS) |
| Regional hosts | `<region>.grpc.solami.dev` (`nyc`, `ams`, `fra`) |
| Scheme for the Rust client | `https://grpc.solami.dev:443` |
| Service | `geyser.Geyser` (Yellowstone), bidi `Subscribe` stream |
| Reflection | enabled |
| HTTP/2 | required (negotiated over TLS/ALPN) |

Source: `/llms.txt` ("grpc.solami.dev for gRPC"), `/docs/endpoints`,
`/docs/grpc`.

## 2. Authentication

* **gRPC uses the `x-token` metadata header** carrying the API key.
* Other transports differ (RPC `?api_key=`, WebSocket `?api_key=`), but gRPC
  is header-only.
* Failure mode for a bad/revoked key: `PERMISSION_DENIED`
  (`invalid api key` / `api key revoked`). A key without gRPC access returns
  `PERMISSION_DENIED: this key does not have gRPC access`.
* Beam/ShredDirect are out of scope for this milestone.

Source: `/docs/grpc` ("Authenticate with your API key on the `x-token`
header"), `/docs/api-keys`, and the live probe below.

**Credential handling in Neurone:** read only from the environment
(`SOLAMI_GRPC_TOKEN`, fallback `SOLAMI_API_KEY`); never from TOML; never logged.

## 3. Rust client implementation path

Two official paths exist:

* the standard `geyser.Geyser` client (Triton `yellowstone-grpc-client`), and
* Solami's own `solami` Rust SDK (`SubscriptionBuilder` "builds a normal
  Yellowstone `SubscribeRequest`").

Neurone uses the **standard Yellowstone client** for full control over the
`SubscribeRequest` (accounts/slots/transactions/blocks-meta/`from_slot`):

| Crate | Version | Why |
| --- | --- | --- |
| `yellowstone-grpc-client` | 14.0.1 | `GeyserGrpcClient` + `SubscribeRequestSink`/`GeyserStream`, `x_token()`, `tls_config()`, `http2_keep_alive_interval()` |
| `yellowstone-grpc-proto` | 13.0.0 | Generated `geyser` / `confirmed_block` types (client 14.0.1 depends on proto `^13.0.0`) |

Confirmed from crates.io metadata: `yellowstone-grpc-client 14.0.1` depends on
`yellowstone-grpc-proto ^13.0.0` and `tonic ^0.14`. Solami's own SDK is
`solami 0.1.58`; it is not required for this milestone.

## 4. `SubscribeRequest` shape (from the embedded descriptor)

```
SubscribeRequest {
  accounts: map<string, SubscribeRequestFilterAccounts>          // id 1
  slots: map<string, SubscribeRequestFilterSlots>                // id 2
  transactions: map<string, SubscribeRequestFilterTransactions>  // id 3
  transactions_status: map<...>                                  // id 10
  blocks: map<string, SubscribeRequestFilterBlocks>              // id 4
  blocks_meta: map<string, SubscribeRequestFilterBlocksMeta>     // id 5
  entry: map<string, SubscribeRequestFilterEntry>                // id 8
  commitment: CommitmentLevel                                    // id 6
  accounts_data_slice: repeated SubscribeRequestAccountsDataSlice// id 7
  ping: SubscribeRequestPing                                     // id 9
  from_slot: uint64                                              // id 11
}

SubscribeRequestFilterAccounts {
  account: repeated string
  owner: repeated string
  filters: repeated AccountFilter { memcmp | datasize | tokenAccountState | lamports }
  nonempty_txn_signature: bool
}

SubscribeRequestFilterTransactions {
  vote: bool; failed: bool; signature: string
  account_include: repeated string
  account_exclude: repeated string
  account_required: repeated string
}

SubscribeRequestFilterSlots { filter_by_commitment: bool; interslot_updates: bool }
```

(The published proto also carries Solami/Triton extensions such as
`cuckoo_accounts_filter`, `token_accounts`, `block_footer`, `bank_id`,
`SubscribeDeshred`, `SubscribeReplayInfo`, and extra RPCs. Neurone does not use
those in Milestone 1.)

**Filters Neurone installs** (`config/default.toml`):

* `accounts` — `owner = [<program ids>]`, plus any explicit `account_addresses`.
  Scoped by owner, so it is **not** a firehose.
* `txs` — `account_include = [<program ids>]`, `vote = false`. Scoped by
  program id.
* `slots` — `filter_by_commitment = false`, `interslot_updates = true`.
* `blocks_meta` — block boundary metadata.

## 5. Commitment and finality

`CommitmentLevel = { PROCESSED=0, CONFIRMED=1, FINALIZED=2 }`.
Neurone defaults to **`processed`** (lowest latency) and makes it configurable.

## 6. Keepalive / ping

The client sends `SubscribeRequest { ping: SubscribeRequestPing { id } }` on the
same bidi stream; the server answers with `SubscribeUpdate.pong`. Neurone sends
a ping every `ingest.ping_interval_ms` (default 15 s) and separately tracks a
`stale_stream_timeout_ms` (default 30 s): if no update arrives in that window it
forces a reconnect.

## 7. `SubscribeUpdate` event types (from the descriptor)

`SubscribeUpdate.update_oneof` is one of
`{ account, slot, transaction, transaction_status, block, ping, pong, block_meta, entry }`
plus `filters: repeated string` and `created_at: google.protobuf.Timestamp`.

Fields Neurone actually consumes:

| Update | Fields used |
| --- | --- |
| `account` | `slot`, `is_startup`, `account.{pubkey, lamports, owner, data, write_version, txn_signature}` |
| `slot` | `slot`, `parent`, `status` (`SLOT_PROCESSED..SLOT_DEAD`) |
| `transaction` | `slot`, `transaction.{signature, index, is_vote, meta.err, transaction.message.account_keys, meta.loaded_{writable,readonly}_addresses}` |
| `block_meta` | `slot`, `block_height`, `block_time`, `executed_transaction_count` |

`ping`/`pong` are counted as keepalive; `block`, `entry` and
`transaction_status` are counted as `invalid_events` (not subscribed).

## 8. Reconnect / error semantics

From `/docs/grpc` and `/docs/errors`:

* **Slot replay:** set `from_slot` and the stream replays history before going
  live. Replay reaches **up to 3,500 slots** back (~23 min). Reconnect with
  `from_slot = last_seen + 1` for a gapless resume.
* `UNAVAILABLE` `all geyser backends unavailable, please retry in a moment` →
  retry with backoff.
* `UNAVAILABLE` `failed to restore subscription on replacement backend, please
  reconnect` → resubscribe from the last slot.
* `UNAVAILABLE` `session expired, please reconnect` → long-lived streams are
  periodically re-authorized.
* `DEADLINE_EXCEEDED` `no initial subscribe request received in time` (120 s
  window) / `INVALID_ARGUMENT` `empty subscribe stream`.
* Backpressure: `grpc_buffer_size` + `grpc_backpressure_action` decide between
  dropping the connection (reconnect from last slot) and dropping newest
  updates. Narrowfilters and drain faster to avoid it.
* Always back off between attempts; rapid reconnects from one IP get rejected.

## 9. Filter limits / quotas (non-PAYG)

| Rule | Limit |
| --- | --- |
| Total filters per request | 25 |
| Addresses in one tx filter `account_include` | 300,000 |
| Addresses in one account filter | 1,000,000 |

A request is a **firehose** (rejected on non-PAYG streams) if any filter is
unscoped: a transactions filter with no `account_include`/`account_required`,
an accounts filter with no account/owner/data filter, an all-entries
subscription, or a blocks filter requesting transactions without
`account_include`. PAYG streams are unrestricted.

Neurone's default filters are all scoped, so they are legal on a plan-included
stream.

## 10. Billing-relevant facts (point-in-time)

`/llms.txt` states gRPC is PAYG at **$0.08/GB** or a standalone **$10/day /
$100/month per connection**, with a 2-day free trial; the authoritative live
numbers come from `GET https://api.solami.dev/pricing`. Neurone treats these as
informational only.

## 11. Live connectivity probe (this environment)

Performed 2026-10-07 from the build host:

* DNS: `grpc.solami.dev` → `181.215.23.23`.
* TCP 443: open.
* TLS: verified, HTTP/2 negotiated (`curl --http2` → `200`, `http_version=2`,
  `ssl_verify_result=0`).
* Real client run (`SOLAMI_GRPC_TOKEN=invalid-test-token cargo run -- run`):
  the process connected, authenticated with `x-token`, and the server replied
  `UNAUTHENTICATED: invalid api key`; Neurone logged the failure, backed off
  (500 → 1000 → 2000 → 4000 ms) and reconnected without routing any events.

This proves the endpoint, TLS, HTTP/2, `x-token` auth path, error handling and
reconnect logic are correct. **No valid Solami credential exists in this
environment**, so live event reception could not be exercised here.
