//! PumpSwap late-market bootstrap from transaction `postTokenBalances`.
//!
//! A PumpSwap pool discovered after launch must be able to establish its
//! current reserves from the already-subscribed transaction stream (bounded,
//! Yellowstone-only, no RPC, no token-account subscription) instead of waiting
//! for a decoded swap. Effective reserves are
//! `base = raw base vault balance` and
//! `quote = raw quote vault balance + virtual_quote_reserves` (signed i128).

use std::sync::Arc;

use neurone::decode::pump_amm::PROGRAM_ID as PAMM_PROGRAM;
use neurone::decode::{DecodedAccount, DecodedSwap, Venue};
use neurone::engine::Engine;
use neurone::events::{
    AccountUpdate, EventKind, MarketKey, NormalizedEvent, TransactionUpdate, VaultBalance,
};
use neurone::market::{MarketState, ReserveState};
use neurone::shutdown::{shutdown_channel, ShutdownHandle};
use neurone::telemetry::Metrics;

fn key(b: u8) -> MarketKey {
    let mut k = [0u8; 32];
    k[0] = b;
    k[31] = 0x99;
    MarketKey(k)
}

fn sig(b: u8) -> [u8; 64] {
    let mut s = [0u8; 64];
    s[0] = b;
    s
}

fn pamm_program() -> MarketKey {
    MarketKey(PAMM_PROGRAM)
}

/// Encode a pump_amm `Pool` account (current layout, incl. signed
/// `virtual_quote_reserves`).
fn encode_pool(
    base_mint: MarketKey,
    quote_mint: MarketKey,
    base_vault: MarketKey,
    quote_vault: MarketKey,
    virtual_quote: i128,
) -> Vec<u8> {
    let mut d = Vec::new();
    d.extend_from_slice(&neurone::decode::pump_amm::POOL_DISC);
    d.push(255); // pool_bump
    d.extend_from_slice(&0u16.to_le_bytes()); // index
    d.extend_from_slice(&[0u8; 32]); // creator
    d.extend_from_slice(base_mint.as_bytes());
    d.extend_from_slice(quote_mint.as_bytes());
    d.extend_from_slice(&[0u8; 32]); // lp_mint
    d.extend_from_slice(base_vault.as_bytes());
    d.extend_from_slice(quote_vault.as_bytes());
    d.extend_from_slice(&0u64.to_le_bytes()); // lp_supply
    d.extend_from_slice(&[0u8; 32]); // coin_creator
    d.push(0); // is_mayhem_mode
    d.push(0); // is_cashback_coin
    d.extend_from_slice(&virtual_quote.to_le_bytes());
    d
}

/// A pool metadata account update that went through the real ingestion
/// boundary (protobuf -> normalize -> decode).
fn pool_account_via_normalizer(
    pool: MarketKey,
    slot: u64,
    wv: u64,
    data: Vec<u8>,
) -> NormalizedEvent {
    use yellowstone_grpc_proto::prelude::{
        subscribe_update::UpdateOneof, SubscribeUpdate, SubscribeUpdateAccount,
        SubscribeUpdateAccountInfo,
    };
    let update = SubscribeUpdate {
        update_oneof: Some(UpdateOneof::Account(SubscribeUpdateAccount {
            account: Some(SubscribeUpdateAccountInfo {
                pubkey: pool.as_bytes().to_vec(),
                lamports: 1,
                owner: PAMM_PROGRAM.to_vec(),
                data,
                write_version: wv,
                txn_signature: None,
                ..Default::default()
            }),
            slot,
            is_startup: false,
            ..Default::default()
        })),
        ..Default::default()
    };
    match neurone::events::normalize(&update, 0) {
        neurone::events::Normalized::Event(e) => e,
        other => panic!("expected event, got {other:?}"),
    }
}

/// Pool metadata as a decoded account (compact form of the same thing).
#[allow(clippy::too_many_arguments)]
fn pool_account(
    pool: MarketKey,
    slot: u64,
    wv: u64,
    base_mint: MarketKey,
    quote_mint: MarketKey,
    base_vault: MarketKey,
    quote_vault: MarketKey,
    virtual_quote: i128,
) -> NormalizedEvent {
    let decoded = DecodedAccount {
        venue: Venue::PumpSwap,
        base_mint: Some(base_mint),
        quote_mint: Some(quote_mint),
        base_reserve: None,
        quote_reserve: None,
        virtual_base_reserve: None,
        virtual_quote_reserve: Some(virtual_quote),
        token_total_supply: None,
        complete: None,
        creator: None,
        pool_base_token_account: Some(base_vault),
        pool_quote_token_account: Some(quote_vault),
    };
    NormalizedEvent::new(EventKind::Account(AccountUpdate {
        pubkey: pool,
        slot,
        owner: Some(pamm_program()),
        lamports: 1,
        data_len: 293,
        data_digest: wv,
        write_version: wv,
        is_startup: false,
        txn_signature: None,
        decoded: Some(decoded),
    }))
}

fn vault_balance(
    account: MarketKey,
    authority: MarketKey,
    mint: MarketKey,
    amount: u64,
) -> VaultBalance {
    VaultBalance {
        account,
        authority,
        mint,
        amount,
    }
}

/// A transaction touching `keys` with the given decoded swaps and post-token
/// balances (no `has_reserve_mutation`, no sweep).
fn tx(
    keys: Vec<MarketKey>,
    slot: u64,
    signature: u8,
    swaps: Vec<DecodedSwap>,
    balances: Vec<VaultBalance>,
) -> NormalizedEvent {
    NormalizedEvent::new(EventKind::Transaction(TransactionUpdate {
        signature: sig(signature),
        slot,
        index: 0,
        is_vote: false,
        success: true,
        keys,
        swaps,
        creates: Vec::new(),
        vault_balances: balances,
        decode_rejected: 0,
        has_reserve_mutation: false,
        has_sweep: false,
    }))
}

fn swap(pool: MarketKey, base: u64, quote: u64) -> DecodedSwap {
    DecodedSwap {
        venue: Venue::PumpSwap,
        market_key: pool,
        base_mint: None,
        quote_mint: None,
        is_buy: true,
        base_amount: 1,
        quote_amount: 1,
        user_quote_amount: 1,
        base_reserve: Some(base),
        quote_reserve: Some(quote),
        virtual_base_reserve: None,
        virtual_quote_reserve: None,
        fee_quote: 0,
        fee_bps: Some(25),
        timestamp: Some(1_700_000_000),
        ix_name: None,
    }
}

struct Rig {
    engine: Engine,
    handles: Vec<tokio::task::JoinHandle<()>>,
    handle: ShutdownHandle,
}

fn start(shards: usize) -> Rig {
    let metrics = Metrics::new(shards, vec![100, 1_000, 10_000, 1_000_000]);
    let (handle, shutdown) = shutdown_channel();
    let (engine, handles) = Engine::start(shards, 4_096, Arc::clone(&metrics), shutdown);
    Rig {
        engine,
        handles,
        handle,
    }
}

impl Rig {
    async fn market(&self, k: MarketKey) -> Option<MarketState> {
        self.engine
            .snapshot()
            .await
            .expect("snapshot")
            .into_iter()
            .flat_map(|s| s.markets)
            .find(|m| m.key == k)
    }

    async fn shutdown(self) {
        self.handle.trigger();
        for h in self.handles {
            let _ = h.await;
        }
    }
}

const BASE_MINT: u8 = 0xB1;
const QUOTE_MINT: u8 = 0xB2;
const BASE_VAULT: u8 = 0xB3;
const QUOTE_VAULT: u8 = 0xB4;

/// (1) Pool metadata is received/decoded from Yellowstone account data.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pool_metadata_decodes_from_yellowstone_account_data() {
    let rig = start(8);
    let pool = key(1);
    let data = encode_pool(
        key(BASE_MINT),
        key(QUOTE_MINT),
        key(BASE_VAULT),
        key(QUOTE_VAULT),
        -12_345,
    );
    rig.engine
        .route(pool_account_via_normalizer(pool, 500_000, 7, data))
        .await
        .unwrap();

    let m = rig.market(pool).await.expect("pool market created");
    assert_eq!(m.venue, Some(Venue::PumpSwap));
    assert_eq!(m.base_mint, Some(key(BASE_MINT)));
    assert_eq!(m.quote_mint, Some(key(QUOTE_MINT)));
    assert_eq!(m.pool_base_token_account, Some(key(BASE_VAULT)));
    assert_eq!(m.pool_quote_token_account, Some(key(QUOTE_VAULT)));
    assert_eq!(m.virtual_quote_reserve, -12_345, "signed i128 preserved");
    assert!(!m.reserves_known);
    assert_eq!(m.reserve_state(500_000, 150), ReserveState::Unknown);
    rig.shutdown().await;
}

/// (2, 12) A late pool bootstraps from `postTokenBalances` with no swap event,
/// no launch history, and no predecessor chain.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_pool_bootstraps_from_post_token_balances_without_swap() {
    let rig = start(8);
    let pool = key(2);
    rig.engine
        .route(pool_account(
            pool,
            600_000,
            1,
            key(BASE_MINT),
            key(QUOTE_MINT),
            key(BASE_VAULT),
            key(QUOTE_VAULT),
            30_000_000_000,
        ))
        .await
        .unwrap();
    assert_eq!(
        rig.market(pool).await.unwrap().reserve_state(600_000, 150),
        ReserveState::Unknown
    );

    let balances = vec![
        vault_balance(key(BASE_VAULT), pool, key(BASE_MINT), 900_000),
        vault_balance(key(QUOTE_VAULT), pool, key(QUOTE_MINT), 5_000_000),
    ];
    rig.engine
        .route(tx(vec![pool], 600_010, 1, Vec::new(), balances))
        .await
        .unwrap();

    let m = rig.market(pool).await.unwrap();
    assert!(m.reserves_known);
    assert_eq!(m.base_reserve, 900_000);
    assert_eq!(
        m.quote_reserve,
        5_000_000 + 30_000_000_000,
        "raw quote + virtual"
    );
    assert_eq!(
        m.last_reserve_slot, 600_010,
        "freshness anchored to the tx slot"
    );
    assert_eq!(m.reserve_state(600_010, 150), ReserveState::Known);
    rig.shutdown().await;
}

/// (3) Base vault must match by exact pubkey AND expected mint.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn base_vault_must_match_pubkey_and_mint() {
    let rig = start(8);
    for (tag, bad_base, bad_mint) in [
        ("wrong base pubkey", key(0xE1), key(BASE_MINT)),
        ("wrong base mint", key(BASE_VAULT), key(0xE2)),
    ] {
        let pool = key(3);
        rig.engine
            .route(pool_account(
                pool,
                100,
                1,
                key(BASE_MINT),
                key(QUOTE_MINT),
                key(BASE_VAULT),
                key(QUOTE_VAULT),
                1_000,
            ))
            .await
            .unwrap();
        let balances = vec![
            vault_balance(bad_base, pool, bad_mint, 900_000),
            vault_balance(key(QUOTE_VAULT), pool, key(QUOTE_MINT), 5_000_000),
        ];
        rig.engine
            .route(tx(
                vec![pool],
                101,
                bad_mint.0[0] ^ 0x5A,
                Vec::new(),
                balances,
            ))
            .await
            .unwrap();
        let m = rig.market(pool).await.unwrap();
        assert!(!m.reserves_known, "{tag}: must not bootstrap");
        assert_eq!(m.reserve_state(101, 150), ReserveState::Unknown, "{tag}");
    }
    rig.shutdown().await;
}

/// (4) Quote vault must match by exact pubkey AND expected mint.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quote_vault_must_match_pubkey_and_mint() {
    let rig = start(8);
    for (tag, bad_quote, bad_mint) in [
        ("wrong quote pubkey", key(0xE3), key(QUOTE_MINT)),
        ("wrong quote mint", key(QUOTE_VAULT), key(0xE4)),
    ] {
        let pool = key(4);
        rig.engine
            .route(pool_account(
                pool,
                100,
                1,
                key(BASE_MINT),
                key(QUOTE_MINT),
                key(BASE_VAULT),
                key(QUOTE_VAULT),
                1_000,
            ))
            .await
            .unwrap();
        let balances = vec![
            vault_balance(key(BASE_VAULT), pool, key(BASE_MINT), 900_000),
            vault_balance(bad_quote, pool, bad_mint, 5_000_000),
        ];
        rig.engine
            .route(tx(
                vec![pool],
                101,
                bad_mint.0[0] ^ 0x5A,
                Vec::new(),
                balances,
            ))
            .await
            .unwrap();
        assert!(
            !rig.market(pool).await.unwrap().reserves_known,
            "{tag}: must not bootstrap"
        );
    }
    rig.shutdown().await;
}

/// (5) Unrelated balances (wrong authority) cannot contaminate the pool.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unrelated_balances_cannot_contaminate_pool() {
    let rig = start(8);
    let pool = key(5);
    rig.engine
        .route(pool_account(
            pool,
            100,
            1,
            key(BASE_MINT),
            key(QUOTE_MINT),
            key(BASE_VAULT),
            key(QUOTE_VAULT),
            1_000,
        ))
        .await
        .unwrap();
    let balances = vec![
        vault_balance(key(BASE_VAULT), key(0xF1), key(BASE_MINT), 900_000),
        vault_balance(key(QUOTE_VAULT), key(0xF1), key(QUOTE_MINT), 5_000_000),
    ];
    rig.engine
        .route(tx(vec![pool], 101, 1, Vec::new(), balances))
        .await
        .unwrap();
    assert!(
        !rig.market(pool).await.unwrap().reserves_known,
        "mismatched authority must not bootstrap"
    );
    rig.shutdown().await;
}

/// (6) Missing one vault balance keeps the market Unknown.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_one_vault_balance_keeps_unknown() {
    let rig = start(8);
    let pool = key(6);
    rig.engine
        .route(pool_account(
            pool,
            100,
            1,
            key(BASE_MINT),
            key(QUOTE_MINT),
            key(BASE_VAULT),
            key(QUOTE_VAULT),
            1_000,
        ))
        .await
        .unwrap();
    let balances = vec![vault_balance(
        key(BASE_VAULT),
        pool,
        key(BASE_MINT),
        900_000,
    )];
    rig.engine
        .route(tx(vec![pool], 101, 1, Vec::new(), balances))
        .await
        .unwrap();
    let m = rig.market(pool).await.unwrap();
    assert!(!m.reserves_known);
    assert_eq!(m.reserve_state(101, 150), ReserveState::Unknown);
    rig.shutdown().await;
}

/// (7) Missing Pool metadata keeps the market Unknown (no fabrication).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_pool_metadata_keeps_unknown() {
    let rig = start(8);
    let pool = key(7);
    let balances = vec![
        vault_balance(key(BASE_VAULT), pool, key(BASE_MINT), 900_000),
        vault_balance(key(QUOTE_VAULT), pool, key(QUOTE_MINT), 5_000_000),
    ];
    rig.engine
        .route(tx(vec![pool], 101, 1, Vec::new(), balances))
        .await
        .unwrap();
    assert!(rig.market(pool).await.is_none());
    rig.shutdown().await;
}

/// (8) Signed negative `virtual_quote_reserves` is applied exactly; a negative
/// effective quote reserve is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_negative_virtual_quote_reserves_is_handled() {
    let rig = start(8);
    let pool = key(8);
    rig.engine
        .route(pool_account(
            pool,
            100,
            1,
            key(BASE_MINT),
            key(QUOTE_MINT),
            key(BASE_VAULT),
            key(QUOTE_VAULT),
            -300,
        ))
        .await
        .unwrap();
    let balances = vec![
        vault_balance(key(BASE_VAULT), pool, key(BASE_MINT), 900),
        vault_balance(key(QUOTE_VAULT), pool, key(QUOTE_MINT), 1_000),
    ];
    rig.engine
        .route(tx(vec![pool], 101, 1, Vec::new(), balances))
        .await
        .unwrap();
    let m = rig.market(pool).await.unwrap();
    assert_eq!(m.quote_reserve, 700);
    assert_eq!(m.reserve_state(101, 150), ReserveState::Known);

    let pool2 = key(9);
    rig.engine
        .route(pool_account(
            pool2,
            100,
            1,
            key(BASE_MINT),
            key(QUOTE_MINT),
            key(BASE_VAULT),
            key(QUOTE_VAULT),
            -2_000,
        ))
        .await
        .unwrap();
    let balances2 = vec![
        vault_balance(key(BASE_VAULT), pool2, key(BASE_MINT), 900),
        vault_balance(key(QUOTE_VAULT), pool2, key(QUOTE_MINT), 1_000),
    ];
    rig.engine
        .route(tx(vec![pool2], 101, 2, Vec::new(), balances2))
        .await
        .unwrap();
    let m2 = rig.market(pool2).await.unwrap();
    assert!(!m2.reserves_known);
    assert_eq!(m2.reserve_state(101, 150), ReserveState::Unknown);
    rig.shutdown().await;
}

/// (9) The existing PumpSwap swap-event path still establishes reserves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn existing_pumpswap_swap_path_still_works() {
    let rig = start(8);
    let pool = key(10);
    rig.engine
        .route(tx(
            vec![pool],
            200,
            1,
            vec![swap(pool, 1_234, 5_678)],
            Vec::new(),
        ))
        .await
        .unwrap();
    let m = rig.market(pool).await.unwrap();
    assert!(m.reserves_known);
    assert_eq!(m.base_reserve, 1_234);
    assert_eq!(m.quote_reserve, 5_678);
    assert_eq!(m.reserve_state(200, 150), ReserveState::Known);
    rig.shutdown().await;
}

/// (10) A swap event wins over the same transaction's vault balances.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn swap_event_is_authoritative_over_vault_balances_in_same_tx() {
    let rig = start(8);
    let pool = key(11);
    rig.engine
        .route(pool_account(
            pool,
            300,
            1,
            key(BASE_MINT),
            key(QUOTE_MINT),
            key(BASE_VAULT),
            key(QUOTE_VAULT),
            1_000,
        ))
        .await
        .unwrap();
    let balances = vec![
        vault_balance(key(BASE_VAULT), pool, key(BASE_MINT), 900_000),
        vault_balance(key(QUOTE_VAULT), pool, key(QUOTE_MINT), 5_000_000),
    ];
    rig.engine
        .route(tx(
            vec![pool],
            301,
            1,
            vec![swap(pool, 1_234, 5_678)],
            balances,
        ))
        .await
        .unwrap();
    let m = rig.market(pool).await.unwrap();
    assert_eq!(m.base_reserve, 1_234, "swap event is authoritative");
    assert_eq!(m.quote_reserve, 5_678);
    rig.shutdown().await;
}

/// (11) Existing stale/invalidated fail-closed semantics are unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_and_invalidated_semantics_unchanged() {
    let rig = start(8);
    let pool = key(12);
    rig.engine
        .route(pool_account(
            pool,
            400,
            1,
            key(BASE_MINT),
            key(QUOTE_MINT),
            key(BASE_VAULT),
            key(QUOTE_VAULT),
            1_000,
        ))
        .await
        .unwrap();
    let balances = vec![
        vault_balance(key(BASE_VAULT), pool, key(BASE_MINT), 900),
        vault_balance(key(QUOTE_VAULT), pool, key(QUOTE_MINT), 1_000),
    ];
    rig.engine
        .route(tx(vec![pool], 401, 1, Vec::new(), balances))
        .await
        .unwrap();
    assert_eq!(
        rig.market(pool).await.unwrap().reserve_state(401, 150),
        ReserveState::Known
    );
    assert_eq!(
        rig.market(pool).await.unwrap().reserve_state(700, 150),
        ReserveState::Stale
    );
    let mut mutation = tx(vec![pool], 402, 2, Vec::new(), Vec::new());
    if let EventKind::Transaction(t) = &mut mutation.kind {
        t.has_reserve_mutation = true;
    }
    rig.engine.route(mutation).await.unwrap();
    assert_eq!(
        rig.market(pool).await.unwrap().reserve_state(402, 150),
        ReserveState::Invalidated
    );
    rig.shutdown().await;
}

/// (13) The ingestion boundary extracts `postTokenBalances` (index -> pubkey,
/// mint, authority, raw amount).
#[test]
fn normalizer_extracts_post_token_balances() {
    use yellowstone_grpc_proto::prelude::{
        subscribe_update::UpdateOneof, SubscribeUpdate, SubscribeUpdateTransaction,
        SubscribeUpdateTransactionInfo, TokenBalance, UiTokenAmount,
    };
    use yellowstone_grpc_proto::solana::storage::confirmed_block::{
        Message, Transaction, TransactionStatusMeta,
    };

    let pool = key(20);
    let base_vault = key(0xC1);
    let quote_vault = key(0xC2);
    let tb = |index: u32, mint: MarketKey, amount: &str| TokenBalance {
        account_index: index,
        mint: mint.to_base58(),
        ui_token_amount: Some(UiTokenAmount {
            ui_amount: 0.0,
            decimals: 6,
            amount: amount.to_string(),
            ui_amount_string: amount.to_string(),
        }),
        owner: pool.to_base58(),
        program_id: "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA".to_string(),
    };
    let update = SubscribeUpdate {
        update_oneof: Some(UpdateOneof::Transaction(SubscribeUpdateTransaction {
            transaction: Some(SubscribeUpdateTransactionInfo {
                signature: vec![1u8; 64],
                is_vote: false,
                transaction: Some(Transaction {
                    signatures: vec![vec![1u8; 64]],
                    message: Some(Message {
                        account_keys: vec![
                            pool.as_bytes().to_vec(),
                            base_vault.as_bytes().to_vec(),
                            quote_vault.as_bytes().to_vec(),
                        ],
                        ..Default::default()
                    }),
                }),
                meta: Some(TransactionStatusMeta {
                    post_token_balances: vec![
                        tb(1, key(BASE_MINT), "123"),
                        tb(2, key(QUOTE_MINT), "456789"),
                    ],
                    ..Default::default()
                }),
                index: 0,
            }),
            slot: 1,
            ..Default::default()
        })),
        ..Default::default()
    };
    match neurone::events::normalize(&update, 0) {
        neurone::events::Normalized::Event(e) => match e.kind {
            EventKind::Transaction(t) => {
                assert_eq!(
                    t.vault_balances,
                    vec![
                        vault_balance(base_vault, pool, key(BASE_MINT), 123),
                        vault_balance(quote_vault, pool, key(QUOTE_MINT), 456_789),
                    ]
                );
            }
            other => panic!("expected transaction, got {other:?}"),
        },
        other => panic!("expected event, got {other:?}"),
    }
}
