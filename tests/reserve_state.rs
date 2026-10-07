//! M2.1 reserve-state and freshness tests (pump.swap-specific semantics).

use std::collections::BTreeMap;
use std::sync::Arc;

use neurone::decode::{DecodedSwap, Venue};
use neurone::engine::Engine;
use neurone::events::{EventKind, MarketKey, NormalizedEvent, TransactionUpdate};
use neurone::market::{MarketState, ReserveState};
use neurone::shutdown::{shutdown_channel, ShutdownHandle};
use neurone::telemetry::Metrics;

fn key(b: u8) -> MarketKey {
    let mut k = [0u8; 32];
    k[0] = b;
    k[31] = 0xAA;
    MarketKey(k)
}

fn swap(market: MarketKey, base: u64, quote: u64, _sig: u8) -> DecodedSwap {
    DecodedSwap {
        venue: Venue::PumpSwap,
        market_key: market,
        base_mint: Some(key(0xF1)),
        quote_mint: Some(key(0xF2)),
        is_buy: true,
        base_amount: 1,
        quote_amount: 1,
        base_reserve: Some(base),
        quote_reserve: Some(quote),
        virtual_quote_reserve: None,
        fee_quote: 0,
        fee_bps: Some(25),
        timestamp: Some(1_700_000_000),
        ix_name: None,
    }
}

fn tx_event(market: MarketKey, slot: u64, sig: u8, s: DecodedSwap) -> NormalizedEvent {
    let mut signature = [0u8; 64];
    signature[0] = sig;
    NormalizedEvent::new(EventKind::Transaction(TransactionUpdate {
        signature,
        slot,
        index: 0,
        is_vote: false,
        success: true,
        keys: vec![market],
        swaps: vec![s],
        creates: Vec::new(),
        decode_rejected: 0,
    }))
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
    async fn markets(&self) -> BTreeMap<MarketKey, MarketState> {
        let mut out = BTreeMap::new();
        for shard in self.engine.snapshot().await.expect("snapshot") {
            for market in shard.markets {
                out.insert(market.key, market);
            }
        }
        out
    }

    async fn shutdown(self) {
        self.handle.trigger();
        for h in self.handles {
            let _ = h.await;
        }
    }
}

#[test]
fn unknown_state_is_represented() {
    let m = MarketState::new(key(1), 0);
    assert!(!m.reserves_known);
    assert_eq!(m.reserve_state(1_000, 150), ReserveState::Unknown);
    assert!(!m.is_reserve_state_fresh(1_000, 150));
}

#[test]
fn stale_state_is_detected() {
    let mut m = MarketState::new(key(1), 0);
    m.reserves_known = true;
    m.last_reserve_slot = 1_000;
    assert_eq!(m.reserve_state(1_150, 150), ReserveState::Known);
    assert_eq!(m.reserve_state(1_151, 150), ReserveState::Stale);
    assert!(!m.is_reserve_state_fresh(2_000, 150));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn first_swap_establishes_reserves_then_newer_replaces() {
    let rig = start(8);
    let market = key(1);
    rig.engine
        .route(tx_event(market, 100, 1, swap(market, 1_000, 5_000, 1)))
        .await
        .unwrap();
    let after_first = rig.markets().await;
    let m = after_first.get(&market).unwrap();
    assert!(m.reserves_known);
    assert_eq!(m.base_reserve, 1_000);
    assert_eq!(m.quote_reserve, 5_000);
    assert_eq!(m.last_reserve_slot, 100);
    assert_eq!(m.reserve_state(100, 150), ReserveState::Known);

    rig.engine
        .route(tx_event(market, 200, 2, swap(market, 2_000, 9_000, 2)))
        .await
        .unwrap();
    let markets = rig.markets().await;
    rig.shutdown().await;
    let m = markets.get(&market).unwrap();
    assert_eq!(m.base_reserve, 2_000);
    assert_eq!(m.quote_reserve, 9_000);
    assert_eq!(m.last_reserve_slot, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn older_swap_cannot_overwrite_newer_reserves() {
    let rig = start(8);
    let market = key(2);
    // Newer first, then an older (out-of-order) event.
    rig.engine
        .route(tx_event(market, 200, 1, swap(market, 2_000, 9_000, 1)))
        .await
        .unwrap();
    rig.engine
        .route(tx_event(market, 150, 2, swap(market, 111, 222, 2)))
        .await
        .unwrap();

    let markets = rig.markets().await;
    rig.shutdown().await;
    let m = markets.get(&market).unwrap();
    assert_eq!(m.last_reserve_slot, 200);
    assert_eq!(m.base_reserve, 2_000, "older reserves must not overwrite");
    assert_eq!(m.quote_reserve, 9_000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replayed_swap_is_idempotent_for_reserves() {
    let rig = start(8);
    let market = key(3);
    let ev = tx_event(market, 300, 7, swap(market, 1_234, 5_678, 7));
    rig.engine.route(ev.clone()).await.unwrap();
    rig.engine.route(ev).await.unwrap();
    let markets = rig.markets().await;
    rig.shutdown().await;
    let m = markets.get(&market).unwrap();
    assert_eq!(m.trade_count, 1);
    assert_eq!(m.base_reserve, 1_234);
    assert_eq!(m.last_reserve_slot, 300);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multiple_pools_keep_independent_reserves() {
    let rig = start(8);
    let a = key(4);
    let b = key(5);
    rig.engine
        .route(tx_event(a, 10, 1, swap(a, 1_000, 5_000, 1)))
        .await
        .unwrap();
    rig.engine
        .route(tx_event(b, 11, 2, swap(b, 2_000, 8_000, 2)))
        .await
        .unwrap();
    let markets = rig.markets().await;
    rig.shutdown().await;
    assert_eq!(markets.get(&a).unwrap().base_reserve, 1_000);
    assert_eq!(markets.get(&b).unwrap().base_reserve, 2_000);
    assert_eq!(markets.get(&a).unwrap().quote_reserve, 5_000);
    assert_eq!(markets.get(&b).unwrap().quote_reserve, 8_000);
}
