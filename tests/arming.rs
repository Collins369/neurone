//! M5 — deterministic pre-arming tests (shard-local, no I/O).

use std::sync::Arc;

use neurone::decode::{CreatedMarket, DecodedAccount, DecodedSwap, Venue};
use neurone::engine::Engine;
use neurone::events::{AccountUpdate, EventKind, MarketKey, NormalizedEvent, TransactionUpdate};
use neurone::market::{MarketState, MarketStatus};
use neurone::pyth;
use neurone::shutdown::{shutdown_channel, ShutdownHandle};
use neurone::strategy::{SolUsdSource, StrategyConfig};
use neurone::telemetry::Metrics;

const PRICE: u64 = 1_000_000_000; // $1000/SOL -> usd_micros == quote_raw

fn key(b: u8) -> MarketKey {
    let mut k = [0u8; 32];
    k[0] = b;
    k[31] = 0x7A;
    MarketKey(k)
}

fn sig(b: u8) -> [u8; 64] {
    let mut s = [0u8; 64];
    s[0] = b;
    s
}

fn cfg() -> StrategyConfig {
    StrategyConfig {
        sol_usd_price_micros: Some(PRICE),
        max_token_age_seconds: None,
        ..StrategyConfig::default()
    }
}

fn cfg_pyth(staleness_ms: u64) -> StrategyConfig {
    StrategyConfig {
        sol_usd_source: SolUsdSource::PythYellowstone,
        sol_usd_max_staleness_ms: staleness_ms,
        max_token_age_seconds: None,
        ..StrategyConfig::default()
    }
}

// A qualifying pump.fun market: mcap = vq*supply/vb = 5e9 ($5000),
// liquidity = 2*real_quote = 1e10 ($10,000), volume 1e9 ($1000).
fn account_event(market: MarketKey, slot: u64, wv: u64) -> NormalizedEvent {
    account_event_with(market, slot, wv, Some(5_000_000_000))
}

fn account_event_with(
    market: MarketKey,
    slot: u64,
    wv: u64,
    real_quote: Option<u128>,
) -> NormalizedEvent {
    let decoded = real_quote.map(|q| DecodedAccount {
        venue: Venue::PumpFun,
        base_mint: Some(key(0xA1)),
        quote_mint: None,
        base_reserve: Some(800_000_000_000_000),
        quote_reserve: Some(q),
        virtual_base_reserve: Some(1_000_000_000_000_000),
        virtual_quote_reserve: Some(1_000_000_000),
        token_total_supply: Some(5_000_000_000_000_000),
        complete: Some(false),
        creator: None,
        pool_base_token_account: None,
        pool_quote_token_account: None,
    });
    NormalizedEvent::new(EventKind::Account(AccountUpdate {
        pubkey: market,
        slot,
        owner: Some(MarketKey(neurone::decode::pumpfun::PROGRAM_ID)),
        lamports: 1,
        data_len: 150,
        data_digest: wv,
        write_version: wv,
        is_startup: false,
        txn_signature: None,
        decoded,
        pyth_sol_usd: None,
    }))
}

#[allow(clippy::too_many_arguments)]
fn swap_event(
    market: MarketKey,
    slot: u64,
    sg: u8,
    vol: u64,
    real_quote: u64,
    vquote: i128,
) -> NormalizedEvent {
    let swap = DecodedSwap {
        venue: Venue::PumpFun,
        market_key: market,
        base_mint: None,
        quote_mint: None,
        is_buy: true,
        base_amount: 10,
        quote_amount: vol,
        user_quote_amount: vol,
        base_reserve: Some(800_000_000_000_000),
        quote_reserve: Some(real_quote),
        virtual_base_reserve: Some(1_000_000_000_000_000),
        virtual_quote_reserve: Some(vquote),
        fee_quote: 0,
        fee_bps: Some(95),
        timestamp: Some(1_700_000_000),
        ix_name: None,
    };
    NormalizedEvent::new(EventKind::Transaction(TransactionUpdate {
        signature: sig(sg),
        slot,
        index: 0,
        is_vote: false,
        success: true,
        keys: vec![market],
        swaps: vec![swap],
        creates: Vec::new(),
        vault_balances: Vec::new(),
        decode_rejected: 0,
        has_reserve_mutation: false,
        has_sweep: false,
    }))
}

/// A normal qualifying trade: $1000 volume, reserves in range.
fn good_swap(market: MarketKey, slot: u64, sg: u8) -> NormalizedEvent {
    swap_event(
        market,
        slot,
        sg,
        1_000_000_000,
        5_000_000_000,
        1_000_000_000,
    )
}

fn create_event(market: MarketKey, slot: u64, sg: u8) -> NormalizedEvent {
    let created = CreatedMarket {
        venue: Venue::PumpFun,
        market_key: market,
        mint: key(0xA1),
        quote_mint: None,
        virtual_base_reserve: Some(1_000_000_000_000_000),
        virtual_quote_reserve: Some(1_000_000_000),
        base_reserve: Some(800_000_000_000_000),
        token_total_supply: Some(5_000_000_000_000_000),
        timestamp: Some(1_700_000_000),
    };
    NormalizedEvent::new(EventKind::Transaction(TransactionUpdate {
        signature: sig(sg),
        slot,
        index: 0,
        is_vote: false,
        success: true,
        keys: vec![market],
        swaps: Vec::new(),
        creates: vec![created],
        vault_balances: Vec::new(),
        decode_rejected: 0,
        has_reserve_mutation: false,
        has_sweep: false,
    }))
}

fn mutation_event(market: MarketKey, slot: u64, sg: u8) -> NormalizedEvent {
    NormalizedEvent::new(EventKind::Transaction(TransactionUpdate {
        signature: sig(sg),
        slot,
        index: 0,
        is_vote: false,
        success: true,
        keys: vec![market],
        swaps: Vec::new(),
        creates: Vec::new(),
        vault_balances: Vec::new(),
        decode_rejected: 0,
        has_reserve_mutation: true,
        has_sweep: false,
    }))
}

fn pyth_account_event(slot: u64, sg: u8) -> NormalizedEvent {
    let update = pyth::SolUsdPriceUpdate {
        price: 100_000_000_000,
        conf: 30_000_000,
        exponent: -8,
        publish_time: 1_791_466_553,
        prev_publish_time: 1_791_466_552,
        ema_price: 100_000_000_000,
        ema_conf: 0,
        posted_slot: slot,
        full_verification: true,
    };
    let _ = sg;
    NormalizedEvent::new(EventKind::Account(AccountUpdate {
        pubkey: MarketKey(pyth::SOL_USD_PRICE_ACCOUNT_BYTES),
        slot,
        owner: Some(MarketKey(pyth::RECEIVER_PROGRAM_ID)),
        lamports: 1,
        data_len: 134,
        data_digest: slot,
        write_version: slot,
        is_startup: false,
        txn_signature: None,
        decoded: None,
        pyth_sol_usd: Some(update),
    }))
}

struct Rig {
    engine: Engine,
    handles: Vec<tokio::task::JoinHandle<()>>,
    handle: ShutdownHandle,
    metrics: Arc<Metrics>,
}

fn start(cfg: StrategyConfig) -> Rig {
    let metrics = Metrics::new(8, vec![100, 1_000, 10_000, 1_000_000]);
    let (handle, shutdown) = shutdown_channel();
    let (engine, handles) =
        Engine::start_with_strategy(8, 4_096, Arc::clone(&metrics), shutdown, Arc::new(cfg), 150);
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

/// Discover + arm a qualifying market.
async fn arm(rig: &Rig, m: MarketKey) {
    rig.engine.route(account_event(m, 900, 1)).await.unwrap();
    rig.engine.route(good_swap(m, 990, 1)).await.unwrap();
}

// ------------------------------------------------------------------ arming ---

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn qualified_market_becomes_armed() {
    let rig = start(cfg());
    let m = key(1);
    arm(&rig, m).await;
    let s = rig.market(m).await;
    assert_eq!(s.status, MarketStatus::Armed);
    let ctx = s.armed.expect("armed context");
    assert_eq!(ctx.armed_slot, 990);
    assert_eq!(ctx.market_version, s.strategy_version);
    assert_eq!(rig.metrics.snapshot().markets_armed, 1);
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unqualified_market_is_not_armed() {
    let rig = start(cfg());
    let m = key(2);
    // Account only: no rolling volume -> not qualified -> not armed.
    rig.engine.route(account_event(m, 900, 1)).await.unwrap();
    let s = rig.market(m).await;
    assert_eq!(s.status, MarketStatus::Observing);
    assert!(s.armed.is_none());
    assert_eq!(rig.metrics.snapshot().markets_armed, 0);
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_state_is_not_armed() {
    let rig = start(cfg());
    let m = key(3);
    // No reserves -> Unknown -> not armed even with volume.
    rig.engine
        .route(account_event_with(m, 900, 1, None))
        .await
        .unwrap();
    rig.engine.route(good_swap(m, 990, 1)).await.unwrap();
    let s = rig.market(m).await;
    assert_ne!(s.status, MarketStatus::Armed);
    assert!(s.armed.is_none());
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_sol_usd_reference_disarms() {
    // 1 ms staleness bound: the reference is stale almost immediately.
    let rig = start(cfg_pyth(1));
    let m = key(4);
    rig.engine
        .route(pyth_account_event(1_000, 1))
        .await
        .unwrap();
    arm(&rig, m).await;
    assert_eq!(rig.market(m).await.status, MarketStatus::Armed);

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    rig.engine.route(good_swap(m, 1_000, 2)).await.unwrap();
    let s = rig.market(m).await;
    assert_eq!(s.status, MarketStatus::Observing);
    assert!(s.armed.is_none());
    assert!(rig.metrics.snapshot().arm_invalidated >= 1);
    rig.shutdown().await;
}

// ------------------------------------------------------------ invalidation ---

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reserve_invalidation_disarms() {
    let rig = start(cfg());
    let m = key(5);
    arm(&rig, m).await;
    assert_eq!(rig.market(m).await.status, MarketStatus::Armed);
    rig.engine.route(mutation_event(m, 995, 2)).await.unwrap();
    let s = rig.market(m).await;
    assert_eq!(s.status, MarketStatus::Observing);
    assert!(s.armed.is_none());
    assert_eq!(rig.metrics.snapshot().arm_invalidated, 1);
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mcap_leaving_bounds_disarms() {
    let rig = start(cfg());
    let m = key(6);
    arm(&rig, m).await;
    // $15,000 mcap -> above the $10,000 ceiling.
    rig.engine
        .route(swap_event(
            m,
            995,
            2,
            1_000_000_000,
            5_000_000_000,
            3_000_000_000,
        ))
        .await
        .unwrap();
    let s = rig.market(m).await;
    assert_eq!(s.status, MarketStatus::Observing);
    assert!(s.armed.is_none());
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reserve_change_leaving_bounds_disarms() {
    let rig = start(cfg());
    let m = key(7);
    arm(&rig, m).await;
    // Curve depth collapses -> outside the liquidity + MCAP bounds.
    rig.engine
        .route(swap_event(
            m,
            995,
            2,
            1_000_000_000,
            5_000_000_000,
            100_000_000,
        ))
        .await
        .unwrap();
    let s = rig.market(m).await;
    assert_eq!(s.status, MarketStatus::Observing);
    assert!(s.armed.is_none());
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn token_age_exceeding_15_minutes_disarms() {
    let cfg = StrategyConfig {
        max_token_age_seconds: Some(900),
        ..cfg()
    };
    let rig = start(cfg);
    let m = key(8);
    rig.engine.route(create_event(m, 900, 1)).await.unwrap();
    rig.engine.route(account_event(m, 900, 2)).await.unwrap();
    rig.engine.route(good_swap(m, 990, 3)).await.unwrap();
    assert_eq!(rig.market(m).await.status, MarketStatus::Armed);
    // 900 s later (2250 slots): older than the maximum age -> disarmed.
    rig.engine
        .route(good_swap(m, 990 + 2_251, 2))
        .await
        .unwrap();
    let s = rig.market(m).await;
    assert_eq!(s.status, MarketStatus::Observing);
    assert!(s.armed.is_none());
    rig.shutdown().await;
}

// --------------------------------------------------------------- re-arming ---

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn version_change_refreshes_without_duplicate_arm() {
    let rig = start(cfg());
    let m = key(9);
    arm(&rig, m).await;
    let first = rig.market(m).await.armed.unwrap();
    // Still qualifying, but the market changed -> refreshed in place.
    rig.engine.route(good_swap(m, 995, 2)).await.unwrap();
    let s = rig.market(m).await;
    assert_eq!(s.status, MarketStatus::Armed);
    let second = s.armed.unwrap();
    assert_eq!(
        second.armed_slot, first.armed_slot,
        "expiry measured from first arm"
    );
    assert!(second.market_version > first.market_version);
    assert_eq!(rig.metrics.snapshot().markets_armed, 1, "no duplicate arm");
    assert!(rig.metrics.snapshot().arm_refreshed >= 1);
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn expired_arm_is_dropped_then_rearmed() {
    let rig = start(cfg()); // max_arm_age_slots = 150
    let m = key(10);
    arm(&rig, m).await; // armed at slot 990
    rig.engine.route(good_swap(m, 990 + 151, 2)).await.unwrap();
    let s = rig.market(m).await;
    assert_eq!(s.status, MarketStatus::Armed);
    assert_eq!(
        s.armed.unwrap().armed_slot,
        990 + 151,
        "fresh arm after expiry"
    );
    assert_eq!(rig.metrics.snapshot().arm_expired, 1);
    assert_eq!(
        rig.metrics.snapshot().markets_armed,
        2,
        "expired then re-armed"
    );
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalidated_armed_market_can_requalify_and_rearm() {
    let rig = start(cfg());
    let m = key(11);
    arm(&rig, m).await;
    rig.engine.route(mutation_event(m, 995, 2)).await.unwrap();
    assert_eq!(rig.market(m).await.status, MarketStatus::Observing);
    // Fresh authoritative account update at/after the mutation slot recovers,
    // and the market re-arms under a new version.
    rig.engine.route(account_event(m, 996, 3)).await.unwrap();
    rig.engine.route(good_swap(m, 997, 3)).await.unwrap();
    let s = rig.market(m).await;
    assert_eq!(s.status, MarketStatus::Armed);
    assert!(rig.metrics.snapshot().markets_armed >= 2);
    rig.shutdown().await;
}

// ---------------------------------------------------------------- multiple ---

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multiple_markets_arm_independently() {
    let rig = start(cfg());
    let a = key(12);
    let b = key(13);
    arm(&rig, a).await;
    arm(&rig, b).await;
    assert_eq!(rig.market(a).await.status, MarketStatus::Armed);
    assert_eq!(rig.market(b).await.status, MarketStatus::Armed);
    assert_eq!(
        rig.metrics.snapshot().markets_armed,
        2,
        "multiple armed allowed"
    );

    // Invalidating one must not touch the other.
    rig.engine.route(mutation_event(a, 1_000, 4)).await.unwrap();
    assert_eq!(rig.market(a).await.status, MarketStatus::Observing);
    assert_eq!(rig.market(b).await.status, MarketStatus::Armed);
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_identical_updates_are_deterministic() {
    let rig = start(cfg());
    let m = key(14);
    arm(&rig, m).await;
    for i in 0..5u8 {
        rig.engine
            .route(good_swap(m, 1_000 + i as u64, 10 + i))
            .await
            .unwrap();
    }
    let s = rig.market(m).await;
    assert_eq!(s.status, MarketStatus::Armed);
    assert_eq!(
        rig.metrics.snapshot().markets_armed,
        1,
        "armed exactly once"
    );
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn missing_sol_usd_reference_is_not_armed() {
    // Pyth mode with no reference yet -> USD valuation unavailable -> fail closed.
    let rig = start(cfg_pyth(120_000));
    let m = key(15);
    rig.engine.route(account_event(m, 900, 1)).await.unwrap();
    rig.engine.route(good_swap(m, 990, 1)).await.unwrap();
    let s = rig.market(m).await;
    assert_eq!(s.status, MarketStatus::Observing);
    assert!(s.armed.is_none());
    assert_eq!(rig.metrics.snapshot().markets_armed, 0);
    rig.shutdown().await;
}

#[test]
fn arm_expiry_boundary_is_exact() {
    let ctx = neurone::market::ArmedContext {
        side: neurone::quote::Side::Buy,
        armed_slot: 1_000,
        armed_ns: 0,
        market_version: 0,
        sol_usd_micros: None,
    };
    assert!(
        !ctx.is_expired(1_150, 150),
        "exactly at the horizon is still valid"
    );
    assert!(
        ctx.is_expired(1_151, 150),
        "one slot past the horizon expires"
    );
    assert!(!ctx.is_expired(999, 150), "earlier slots are not expired");
}
