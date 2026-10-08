//! Late-observed market semantics.
//!
//! A market discovered *after* launch must be able to bootstrap from the live
//! authoritative Yellowstone state (a bonding-curve account update or a decoded
//! swap) and then track continuously — without observing its launch, and
//! without any historical predecessor chain. `previous_event_not_contiguous`
//! is a validation-methodology concept (see `src/validate.rs`); it must not, by
//! itself, make a runtime market unusable.

use std::sync::Arc;

use neurone::decode::{CreatedMarket, DecodedAccount, DecodedSwap, Venue};
use neurone::engine::Engine;
use neurone::events::{AccountUpdate, EventKind, MarketKey, NormalizedEvent, TransactionUpdate};
use neurone::market::{MarketState, ReserveState};
use neurone::quote::{self, QuoteError, Side};
use neurone::shutdown::{shutdown_channel, ShutdownHandle};
use neurone::telemetry::Metrics;

fn key(b: u8) -> MarketKey {
    let mut k = [0u8; 32];
    k[0] = b;
    k[31] = 0x77;
    MarketKey(k)
}

fn signature(sig: u8) -> [u8; 64] {
    let mut s = [0u8; 64];
    s[0] = sig;
    s
}

/// An authoritative pump.fun bonding-curve account update (carries reserves).
fn curve_account(
    market: MarketKey,
    wv: u64,
    slot: u64,
    vbase: u64,
    vquote: i128,
) -> NormalizedEvent {
    let decoded = DecodedAccount {
        venue: Venue::PumpFun,
        base_mint: None,
        quote_mint: None,
        base_reserve: Some(800_000_000),
        quote_reserve: Some(5_000_000_000),
        virtual_base_reserve: Some(vbase as u128),
        virtual_quote_reserve: Some(vquote),
        token_total_supply: Some(1_000_000_000_000_000),
        complete: Some(false),
        creator: None,
        pool_base_token_account: None,
        pool_quote_token_account: None,
    };
    NormalizedEvent::new(EventKind::Account(AccountUpdate {
        pubkey: market,
        slot,
        owner: Some(key(0xF0)),
        lamports: 1,
        data_len: 150,
        data_digest: wv,
        write_version: wv,
        is_startup: false,
        txn_signature: None,
        decoded: Some(decoded),
    }))
}

/// A successful pump.fun trade carrying reserves (a decoded swap). The reserves
/// are whatever the event says — no predecessor is required.
fn swap_tx(
    market: MarketKey,
    slot: u64,
    sig: u8,
    base: u64,
    quote: u64,
    vquote: i128,
) -> NormalizedEvent {
    let swap = DecodedSwap {
        venue: Venue::PumpFun,
        market_key: market,
        base_mint: Some(market),
        quote_mint: None,
        is_buy: true,
        base_amount: 10,
        quote_amount: 5,
        user_quote_amount: 5,
        base_reserve: Some(base),
        quote_reserve: Some(quote),
        virtual_base_reserve: Some(1_073_000_000_000_000),
        virtual_quote_reserve: Some(vquote),
        fee_quote: 0,
        fee_bps: Some(95),
        timestamp: Some(1_700_000_000),
        ix_name: None,
    };
    NormalizedEvent::new(EventKind::Transaction(TransactionUpdate {
        signature: signature(sig),
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

/// A transaction that merely *touches* the market: it carries no decoded swap
/// and no account state, so it must not establish reserves.
fn touch_tx(market: MarketKey, slot: u64, sig: u8) -> NormalizedEvent {
    NormalizedEvent::new(EventKind::Transaction(TransactionUpdate {
        signature: signature(sig),
        slot,
        index: 0,
        is_vote: false,
        success: true,
        keys: vec![market],
        swaps: Vec::new(),
        creates: Vec::new(),
        decode_rejected: 0,
        vault_balances: Vec::new(),
        has_reserve_mutation: false,
        has_sweep: false,
    }))
}

/// A create event: seeds identity and (untrusted) virtual reserves but does NOT
/// establish a trustworthy current reserve state (`reserves_known` stays false).
fn create_tx(market: MarketKey, slot: u64, sig: u8) -> NormalizedEvent {
    let created = CreatedMarket {
        venue: Venue::PumpFun,
        market_key: market,
        mint: key(0xE1),
        quote_mint: None,
        virtual_base_reserve: Some(1_073_000_000_000_000),
        virtual_quote_reserve: Some(30_000_000_000),
        base_reserve: Some(793_100_000_000_000),
        token_total_supply: Some(1_000_000_000_000_000),
        timestamp: Some(1_700_000_000),
    };
    NormalizedEvent::new(EventKind::Transaction(TransactionUpdate {
        signature: signature(sig),
        slot,
        index: 0,
        is_vote: false,
        success: true,
        keys: vec![market],
        swaps: Vec::new(),
        creates: vec![created],
        decode_rejected: 0,
        vault_balances: Vec::new(),
        has_reserve_mutation: false,
        has_sweep: false,
    }))
}

/// A genuine non-trade reserve mutation (synthetic; not a fee sweep).
fn reserve_mutation_tx(market: MarketKey, slot: u64, sig: u8) -> NormalizedEvent {
    NormalizedEvent::new(EventKind::Transaction(TransactionUpdate {
        signature: signature(sig),
        slot,
        index: 0,
        is_vote: false,
        success: true,
        keys: vec![market],
        swaps: Vec::new(),
        creates: Vec::new(),
        decode_rejected: 0,
        vault_balances: Vec::new(),
        has_reserve_mutation: true,
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

/// (1, 2, 4) A market already live on chain, first observed via a later
/// authoritative bonding-curve account update, bootstraps to `Known`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_market_bootstraps_from_authoritative_account_update() {
    let rig = start(8);
    let m = key(1);
    // No launch, no history — the first thing we ever see is a curve account
    // update at a slot long after the token existed.
    rig.engine
        .route(curve_account(m, 7, 900_000, 1_000, 2_000))
        .await
        .unwrap();

    let state = rig
        .market(m)
        .await
        .expect("market created from account update");
    assert!(
        state.reserves_known,
        "authoritative account establishes reserves"
    );
    assert_eq!(state.reserve_state(900_000, 150), ReserveState::Known);
    assert_eq!(
        state.last_reserve_slot, 900_000,
        "freshness anchored to the account's own slot"
    );
    // Quotable from the current state; no predecessor was required.
    assert_ne!(
        quote::quote(&state, Side::Sell, 1_000, 25, 900_000, 150),
        Err(QuoteError::ReservesUnknown)
    );
    rig.shutdown().await;
}

/// (2, 3, 6) A late market discovered via a swap bootstraps to `Known` and then
/// tracks continuously across arbitrarily discontinuous successors — proving
/// no launch event and no predecessor chain are required at runtime.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_market_requires_no_launch_or_predecessor_chain() {
    let rig = start(8);
    let m = key(2);
    // First observation: a mid-life trade. Reserves are established from it.
    rig.engine
        .route(swap_tx(m, 500_000, 1, 900, 6_000, 30_000))
        .await
        .unwrap();
    let first = rig.market(m).await.expect("market created from swap");
    assert!(first.reserves_known);
    assert_eq!(first.reserve_state(500_000, 150), ReserveState::Known);

    // A successor whose reserves are wildly discontinuous with the predecessor
    // (as if many unobserved trades happened in between). This is exactly the
    // condition the M3 parity validator calls `previous_event_not_contiguous`,
    // yet the runtime must keep tracking.
    rig.engine
        .route(swap_tx(m, 500_010, 2, 111_111, 999_999, 77_777))
        .await
        .unwrap();
    let next = rig.market(m).await.expect("market present");
    assert_eq!(next.base_reserve, 111_111, "successor state applied");
    assert_eq!(next.quote_reserve, 999_999);
    assert_eq!(next.reserve_state(500_010, 150), ReserveState::Known);
    assert!(next.invalidated_at_slot.is_none());
    rig.shutdown().await;
}

/// (4) A tracked market without a trustworthy current reserve state stays
/// non-tradable (never falsely `Known`), and a non-state transaction does not
/// fabricate a market at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn insufficient_current_state_is_not_falsely_known() {
    let rig = start(8);
    // A transaction that only touches a never-seen market must not create one.
    let phantom = key(3);
    rig.engine
        .route(touch_tx(phantom, 700_000, 1))
        .await
        .unwrap();
    assert!(
        rig.market(phantom).await.is_none(),
        "a touch fabricates no market"
    );

    // A create seeds identity but not trust: reserves are not "known" yet.
    let m = key(30);
    rig.engine.route(create_tx(m, 700_000, 1)).await.unwrap();

    let state = rig.market(m).await.expect("market observed");
    assert!(
        !state.reserves_known,
        "create seeds no trustworthy reserves"
    );
    assert_eq!(state.reserve_state(700_000, 150), ReserveState::Unknown);
    assert_eq!(
        quote::quote(&state, Side::Sell, 1_000, 25, 700_000, 150),
        Err(QuoteError::ReservesUnknown)
    );

    // Only an authoritative update makes it Known.
    rig.engine
        .route(curve_account(m, 1, 700_005, 1_000, 2_000))
        .await
        .unwrap();
    assert_eq!(
        rig.market(m).await.unwrap().reserve_state(700_005, 150),
        ReserveState::Known
    );
    rig.shutdown().await;
}

/// (5, 7) After bootstrap, a genuine state gap still fails closed: a non-trade
/// reserve mutation invalidates, and freshness still ages to `Stale`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_bootstrap_gap_still_fails_closed() {
    let rig = start(8);
    let m = key(4);
    rig.engine
        .route(curve_account(m, 1, 100, 1_000, 2_000))
        .await
        .unwrap();
    assert_eq!(
        rig.market(m).await.unwrap().reserve_state(100, 150),
        ReserveState::Known
    );

    // Freshness ages out.
    assert_eq!(
        rig.market(m).await.unwrap().reserve_state(400, 150),
        ReserveState::Stale
    );

    // A genuine reserve mutation invalidates until a fresh authoritative update.
    rig.engine
        .route(reserve_mutation_tx(m, 200, 9))
        .await
        .unwrap();
    let invalidated = rig.market(m).await.unwrap();
    assert_eq!(
        invalidated.reserve_state(200, 150),
        ReserveState::Invalidated
    );
    assert_eq!(
        quote::quote(&invalidated, Side::Sell, 1_000, 25, 200, 150),
        Err(QuoteError::StateInvalidated)
    );
    assert_eq!(rig.metrics.snapshot().reserve_invalidations, 1);

    // A fresh authoritative account update recovers it.
    rig.engine
        .route(curve_account(m, 2, 201, 1_100, 2_100))
        .await
        .unwrap();
    let recovered = rig.market(m).await.unwrap();
    assert_eq!(recovered.reserve_state(201, 150), ReserveState::Known);
    assert!(recovered.invalidated_at_slot.is_none());
    assert_eq!(rig.metrics.snapshot().reserve_revalidations, 1);
    rig.shutdown().await;
}

/// (6) A discontinuous successor does not, by itself, invalidate a runtime
/// market whose current authoritative state is valid.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discontinuous_successor_does_not_invalidate_runtime_market() {
    let rig = start(8);
    let m = key(5);
    // Bootstrap late from the account stream.
    rig.engine
        .route(curve_account(m, 1, 100, 1_000, 2_000))
        .await
        .unwrap();
    // A swap whose reserves do not chain to any predecessor.
    rig.engine
        .route(swap_tx(m, 101, 1, 424_242, 3_141_592, 8_675_309))
        .await
        .unwrap();

    let state = rig.market(m).await.unwrap();
    assert_eq!(state.reserve_state(101, 150), ReserveState::Known);
    assert!(state.invalidated_at_slot.is_none());
    assert_eq!(rig.metrics.snapshot().reserve_invalidations, 0);
    rig.shutdown().await;
}
