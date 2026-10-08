//! M3.3 — non-trade reserve mutation (pump.fun fee sweep) handling.
//!
//! Verifies that a sweep invalidates market state, that stale state cannot be
//! quoted, and that a fresh authoritative account update at a later slot
//! re-establishes valid state.

use std::sync::Arc;

use neurone::decode::{DecodedAccount, DecodedSwap, Venue};
use neurone::engine::Engine;
use neurone::events::{AccountUpdate, EventKind, MarketKey, NormalizedEvent, TransactionUpdate};
use neurone::market::{MarketState, ReserveState};
use neurone::quote::{self, QuoteError, Side};
use neurone::shutdown::{shutdown_channel, ShutdownHandle};
use neurone::telemetry::Metrics;

fn key(b: u8) -> MarketKey {
    let mut k = [0u8; 32];
    k[0] = b;
    k[31] = 0x5A;
    MarketKey(k)
}

fn account_event(k: MarketKey, wv: u64, slot: u64) -> NormalizedEvent {
    // A decoded bonding-curve account so `reserves_known` becomes true.
    let decoded = DecodedAccount {
        venue: Venue::PumpFun,
        base_mint: None,
        quote_mint: None,
        base_reserve: Some(800_000_000),
        quote_reserve: Some(5_000_000_000),
        virtual_base_reserve: Some(1_073_000_000_000_000),
        virtual_quote_reserve: Some(30_000_000_000),
        token_total_supply: Some(1_000_000_000_000_000),
        complete: Some(false),
        creator: None,
        pool_base_token_account: None,
        pool_quote_token_account: None,
    };
    NormalizedEvent::new(EventKind::Account(AccountUpdate {
        pubkey: k,
        slot,
        owner: Some(key(0xF0)),
        lamports: 1,
        data_len: 125,
        data_digest: wv,
        write_version: wv,
        is_startup: false,
        txn_signature: None,
        decoded: Some(decoded),
        pyth_sol_usd: None,
    }))
}

/// A synthetic transaction carrying a *genuine* non-trade reserve mutation.
///
/// Exercises the retained invalidation path directly. This is intentionally
/// **not** a fee sweep: sweeps are proven not to be reserve mutations and must
/// not reach this path (see `protocol_fee_sweep_does_not_invalidate_*`).
fn mutation_event(sig: u8, slot: u64, keys: Vec<MarketKey>) -> NormalizedEvent {
    let mut signature = [0u8; 64];
    signature[0] = sig;
    NormalizedEvent::new(EventKind::Transaction(TransactionUpdate {
        signature,
        slot,
        index: 0,
        is_vote: false,
        success: true,
        keys,
        swaps: Vec::new(),
        creates: Vec::new(),
        decode_rejected: 0,
        vault_balances: Vec::new(),
        has_reserve_mutation: true,
        has_sweep: false,
    }))
}

/// A successful pump.fun trade carrying fresh (authoritative) reserves.
fn swap_event(market: MarketKey, slot: u64, sig: u8) -> NormalizedEvent {
    let mut signature = [0u8; 64];
    signature[0] = sig;
    let swap = DecodedSwap {
        venue: Venue::PumpFun,
        market_key: market,
        base_mint: Some(market),
        quote_mint: None,
        is_buy: true,
        base_amount: 10,
        quote_amount: 5,
        user_quote_amount: 5,
        base_reserve: Some(900_000_000),
        quote_reserve: Some(6_000_000_000),
        virtual_base_reserve: Some(1_073_000_000_000_000),
        virtual_quote_reserve: Some(30_000_000_000),
        fee_quote: 0,
        fee_bps: Some(95),
        timestamp: Some(1_700_000_000),
        ix_name: None,
    };
    NormalizedEvent::new(EventKind::Transaction(TransactionUpdate {
        signature,
        slot,
        index: 0,
        is_vote: false,
        success: true,
        keys: vec![market],
        swaps: vec![swap],
        creates: Vec::new(),
        decode_rejected: 0,
        vault_balances: Vec::new(),
        has_reserve_mutation: false,
        has_sweep: false,
    }))
}

struct Rig {
    engine: Engine,
    handles: Vec<tokio::task::JoinHandle<()>>,
    handle: ShutdownHandle,
    metrics: Arc<Metrics>,
}

fn start(shards: usize) -> Rig {
    let metrics = Metrics::new(shards, vec![100, 1_000, 10_000, 1_000_000]);
    let (handle, shutdown) = shutdown_channel();
    let (engine, handles) = Engine::start(shards, 4_096, Arc::clone(&metrics), shutdown);
    Rig {
        engine,
        handles,
        handle,
        metrics,
    }
}

impl Rig {
    async fn market(&self, k: MarketKey) -> MarketState {
        self.engine
            .snapshot()
            .await
            .expect("snapshot")
            .into_iter()
            .flat_map(|s| s.markets)
            .find(|m| m.key == k)
            .expect("market present")
    }

    async fn shutdown(self) {
        self.handle.trigger();
        for h in self.handles {
            let _ = h.await;
        }
    }
}

/// 1 + 2: a genuine non-trade reserve mutation -> (no fresh state) =>
/// invalidated and not quotable. (A fee sweep is *not* such a mutation; see
/// `protocol_fee_sweep_does_not_invalidate_known_market`.)
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reserve_mutation_invalidates_state_and_blocks_quotes() {
    let rig = start(8);
    let m = key(1);
    rig.engine.route(account_event(m, 1, 100)).await.unwrap();
    rig.engine
        .route(mutation_event(9, 200, vec![m]))
        .await
        .unwrap();

    let state = rig.market(m).await;
    assert_eq!(state.reserve_state(200, 150), ReserveState::Invalidated);
    assert!(state.invalidated_at_slot.is_some());
    // A quote against the invalidated state must be refused.
    let q = quote::quote(&state, Side::Sell, 1_000, 25, 200, 150);
    assert_eq!(q, Err(QuoteError::StateInvalidated));
    rig.shutdown().await;
}

/// 3: trade -> sweep -> fresh account state (slot >= mutation) re-validates.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fresh_account_state_revalidates_market() {
    let rig = start(8);
    let m = key(2);
    rig.engine.route(account_event(m, 1, 100)).await.unwrap();
    rig.engine
        .route(mutation_event(1, 200, vec![m]))
        .await
        .unwrap();
    assert_eq!(
        rig.market(m).await.reserve_state(200, 150),
        ReserveState::Invalidated
    );
    // Fresh authoritative update at a later slot clears invalidation.
    rig.engine.route(account_event(m, 2, 201)).await.unwrap();
    let state = rig.market(m).await;
    assert_eq!(state.reserve_state(201, 150), ReserveState::Known);
    assert!(state.invalidated_at_slot.is_none());
    rig.shutdown().await;
}

/// The invalidation recovery is observable end to end: a sweep bumps
/// `reserve_invalidations`, a fresh authoritative account update bumps
/// `reserve_revalidations`, and the market returns to `Known`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovery_is_observable_via_metrics() {
    let rig = start(8);
    let m = key(7);
    rig.engine.route(account_event(m, 1, 100)).await.unwrap();
    // A snapshot round-trips the shard, so every prior event is applied.
    let _ = rig.market(m).await;
    rig.engine
        .route(mutation_event(1, 200, vec![m]))
        .await
        .unwrap();
    let _ = rig.market(m).await;
    assert_eq!(rig.metrics.snapshot().reserve_invalidations, 1);
    assert_eq!(rig.metrics.snapshot().reserve_revalidations, 0);

    // Fresh authoritative account update at/after the mutation slot recovers.
    rig.engine.route(account_event(m, 2, 201)).await.unwrap();
    let _ = rig.market(m).await;
    let s = rig.metrics.snapshot();
    assert_eq!(s.reserve_revalidations, 1);
    assert_eq!(
        rig.market(m).await.reserve_state(201, 150),
        ReserveState::Known
    );
    rig.shutdown().await;
}

/// 7: an account update from *before* the mutation must not re-validate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_account_update_cannot_revalidate() {
    let rig = start(8);
    let m = key(3);
    rig.engine.route(account_event(m, 1, 100)).await.unwrap();
    rig.engine
        .route(mutation_event(1, 200, vec![m]))
        .await
        .unwrap();
    // An older-slot update (delivery reorder) must not clear invalidation.
    rig.engine.route(account_event(m, 2, 199)).await.unwrap();
    assert_eq!(
        rig.market(m).await.reserve_state(200, 150),
        ReserveState::Invalidated
    );
    rig.shutdown().await;
}

/// 9: multiple mutations before refresh keep the state invalidated.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multiple_mutations_remain_invalidated() {
    let rig = start(8);
    let m = key(4);
    rig.engine.route(account_event(m, 1, 100)).await.unwrap();
    rig.engine
        .route(mutation_event(1, 200, vec![m]))
        .await
        .unwrap();
    rig.engine
        .route(mutation_event(2, 201, vec![m]))
        .await
        .unwrap();
    let state = rig.market(m).await;
    assert_eq!(state.invalidated_at_slot, Some(201));
    assert_eq!(state.reserve_state(201, 150), ReserveState::Invalidated);
    rig.shutdown().await;
}

/// A swap alone must NOT clear invalidation: only a fresh authoritative account
/// update recovers the market (the fail-closed safety constraint).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn swap_alone_does_not_recover_invalidated_state() {
    let rig = start(8);
    let m = key(8);
    rig.engine.route(account_event(m, 1, 100)).await.unwrap();
    rig.engine
        .route(mutation_event(1, 200, vec![m]))
        .await
        .unwrap();
    // A later trade on the curve must not flip the market back to quotable.
    rig.engine.route(swap_event(m, 201, 9)).await.unwrap();
    let state = rig.market(m).await;
    assert_eq!(state.reserve_state(201, 150), ReserveState::Invalidated);
    assert!(state.invalidated_at_slot.is_some());
    assert_eq!(rig.metrics.snapshot().reserve_revalidations, 0);
    rig.shutdown().await;
}

/// 5: invalidate -> refresh -> re-arm (version advances, quote allowed again).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn state_version_advances_and_market_can_rearm() {
    let rig = start(8);
    let m = key(5);
    rig.engine.route(account_event(m, 1, 100)).await.unwrap();
    let v0 = rig.market(m).await.state_version;
    rig.engine
        .route(mutation_event(1, 200, vec![m]))
        .await
        .unwrap();
    let v1 = rig.market(m).await.state_version;
    assert!(v1 > v0);
    rig.engine.route(account_event(m, 2, 202)).await.unwrap();
    let state = rig.market(m).await;
    assert_eq!(state.reserve_state(202, 150), ReserveState::Known);
    assert!(state.state_version > v1);
    rig.shutdown().await;
}

/// A pump.fun fee sweep built from the deployed program's own Anchor logs and
/// normalised through the real ingestion boundary, keyed to `market`.
fn sweep_transaction_event(
    market: MarketKey,
    slot: u64,
    sig: u8,
    creator: bool,
) -> NormalizedEvent {
    use yellowstone_grpc_proto::prelude::{
        subscribe_update::UpdateOneof, SubscribeUpdate, SubscribeUpdateTransaction,
        SubscribeUpdateTransactionInfo,
    };
    use yellowstone_grpc_proto::solana::storage::confirmed_block::{Message, Transaction};

    let mut signature = vec![0u8; 64];
    signature[0] = sig;
    let line = if creator {
        "Program log: Instruction: SweepCreatorFee"
    } else {
        "Program log: Instruction: SweepProtocolFee"
    };
    let update = SubscribeUpdate {
        update_oneof: Some(UpdateOneof::Transaction(SubscribeUpdateTransaction {
            transaction: Some(SubscribeUpdateTransactionInfo {
                signature: signature.clone(),
                is_vote: false,
                transaction: Some(Transaction {
                    signatures: vec![signature],
                    message: Some(Message {
                        account_keys: vec![market.as_bytes().to_vec()],
                        ..Default::default()
                    }),
                }),
                meta: Some(yellowstone_grpc_proto::prelude::TransactionStatusMeta {
                    log_messages: vec![
                        "Program 6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P invoke [1]".into(),
                        line.into(),
                    ],
                    ..Default::default()
                }),
                index: 0,
            }),
            slot,
            ..Default::default()
        })),
        ..Default::default()
    };
    match neurone::events::normalize(&update, 0) {
        neurone::events::Normalized::Event(e) => e,
        other => panic!("expected event, got {other:?}"),
    }
}

fn classify(ev: &NormalizedEvent) -> (bool, bool) {
    match &ev.kind {
        EventKind::Transaction(t) => (t.has_sweep, t.has_reserve_mutation),
        _ => panic!("expected transaction"),
    }
}

/// A fee sweep is observed (`has_sweep`) but is **not** a reserve mutation; a
/// normal trade trips neither. (The M3.3 on-chain proof: sweeps only move fees.)
#[test]
fn normalizer_classifies_sweep_as_observability_not_mutation() {
    let (has_sweep, has_mutation) = classify(&sweep_transaction_event(key(12), 1, 1, false));
    assert!(has_sweep, "protocol sweep must be observed");
    assert!(!has_mutation, "a sweep is not a reserve mutation");

    let (has_sweep, has_mutation) = classify(&sweep_transaction_event(key(13), 1, 2, true));
    assert!(has_sweep, "creator sweep must be observed");
    assert!(!has_mutation, "a sweep is not a reserve mutation");
}

/// A protocol-fee sweep must NOT invalidate a KNOWN market: it is a fee
/// transfer, so the state stays `Known` and quotable and no invalidation is
/// counted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn protocol_fee_sweep_does_not_invalidate_known_market() {
    let rig = start(8);
    let m = key(10);
    rig.engine.route(account_event(m, 1, 100)).await.unwrap();
    assert_eq!(
        rig.market(m).await.reserve_state(100, 150),
        ReserveState::Known
    );

    let ev = sweep_transaction_event(m, 200, 21, false);
    assert_eq!(classify(&ev), (true, false));
    rig.engine.route(ev).await.unwrap();

    let state = rig.market(m).await;
    assert_eq!(state.reserve_state(200, 150), ReserveState::Known);
    assert!(state.invalidated_at_slot.is_none());
    // The market is no longer blocked by invalidation (a sweep is not a mutation).
    assert_ne!(
        quote::quote(&state, Side::Sell, 1_000, 25, 200, 150),
        Err(QuoteError::StateInvalidated)
    );
    assert_eq!(rig.metrics.snapshot().reserve_invalidations, 0);
    rig.shutdown().await;
}

/// A creator-fee sweep must NOT invalidate a KNOWN market (same proof).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn creator_fee_sweep_does_not_invalidate_known_market() {
    let rig = start(8);
    let m = key(11);
    rig.engine.route(account_event(m, 1, 100)).await.unwrap();
    assert_eq!(
        rig.market(m).await.reserve_state(100, 150),
        ReserveState::Known
    );

    let ev = sweep_transaction_event(m, 200, 22, true);
    assert_eq!(classify(&ev), (true, false));
    rig.engine.route(ev).await.unwrap();

    let state = rig.market(m).await;
    assert_eq!(state.reserve_state(200, 150), ReserveState::Known);
    assert!(state.invalidated_at_slot.is_none());
    assert_ne!(
        quote::quote(&state, Side::Sell, 1_000, 25, 200, 150),
        Err(QuoteError::StateInvalidated)
    );
    assert_eq!(rig.metrics.snapshot().reserve_invalidations, 0);
    rig.shutdown().await;
}
