//! M2 state tests: protocol-derived state, venue separation, volume, and
//! determinism through the real sharded engine.

use std::sync::Arc;

use neurone::decode::{CreatedMarket, DecodedAccount, DecodedSwap, Venue};
use neurone::engine::Engine;
use neurone::events::{AccountUpdate, EventKind, MarketKey, NormalizedEvent, TransactionUpdate};
use neurone::market::MarketState;
use neurone::shutdown::{shutdown_channel, ShutdownHandle};
use neurone::telemetry::Metrics;

fn key(b: u8) -> MarketKey {
    let mut k = [0u8; 32];
    k[0] = b;
    k[31] = 0xEE;
    MarketKey(k)
}

fn account_event(
    k: MarketKey,
    decoded: Option<DecodedAccount>,
    wv: u64,
    slot: u64,
) -> NormalizedEvent {
    NormalizedEvent::new(EventKind::Account(AccountUpdate {
        pubkey: k,
        slot,
        owner: Some(key(0xFF)),
        lamports: 1,
        data_len: 125,
        data_digest: wv,
        write_version: wv,
        is_startup: false,
        txn_signature: None,
        decoded,
    }))
}

fn swap(
    _sig: u8,
    market: MarketKey,
    venue: Venue,
    is_buy: bool,
    quote: u64,
    base: u64,
) -> DecodedSwap {
    DecodedSwap {
        venue,
        market_key: market,
        base_mint: Some(market),
        quote_mint: None,
        is_buy,
        base_amount: base,
        quote_amount: quote,
        user_quote_amount: quote,
        base_reserve: Some(base + 1_000),
        quote_reserve: Some(quote + 1_000),
        virtual_base_reserve: None,
        virtual_quote_reserve: Some(30_000_000_000),
        fee_quote: quote / 100,
        fee_bps: Some(95),
        timestamp: Some(1_700_000_000),
        ix_name: None,
    }
}

fn swap_event(sig: u8, slot: u64, s: DecodedSwap) -> NormalizedEvent {
    let mut signature = [0u8; 64];
    signature[0] = sig;
    NormalizedEvent::new(EventKind::Transaction(TransactionUpdate {
        signature,
        slot,
        index: 0,
        is_vote: false,
        success: true,
        keys: vec![s.market_key],
        swaps: vec![s],
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
    async fn markets(&self) -> std::collections::BTreeMap<MarketKey, MarketState> {
        let mut out = std::collections::BTreeMap::new();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn venues_are_tagged_and_separated_per_market() {
    let rig = start(8);
    let curve = key(1);
    let pool = key(2);

    let curve_state = DecodedAccount {
        venue: Venue::PumpFun,
        base_mint: Some(key(11)),
        quote_mint: None,
        base_reserve: Some(800_000_000),
        quote_reserve: Some(5_000_000_000),
        virtual_base_reserve: Some(1_073_000_000_000_000),
        virtual_quote_reserve: Some(30_000_000_000),
        token_total_supply: Some(1_000_000_000_000_000),
        complete: Some(false),
        creator: Some(key(12)),
        pool_base_token_account: None,
        pool_quote_token_account: None,
    };
    let pool_state = DecodedAccount {
        venue: Venue::PumpSwap,
        base_mint: Some(key(21)),
        quote_mint: Some(key(22)),
        base_reserve: None,
        quote_reserve: None,
        virtual_base_reserve: None,
        virtual_quote_reserve: Some(-1),
        token_total_supply: None,
        complete: None,
        creator: Some(key(23)),
        pool_base_token_account: Some(key(24)),
        pool_quote_token_account: Some(key(25)),
    };

    rig.engine
        .route(account_event(curve, Some(curve_state), 1, 10))
        .await
        .unwrap();
    rig.engine
        .route(account_event(pool, Some(pool_state), 1, 10))
        .await
        .unwrap();

    let markets = rig.markets().await;
    rig.shutdown().await;

    let c = markets.get(&curve).unwrap();
    assert_eq!(c.venue, Some(Venue::PumpFun));
    assert_eq!(c.base_mint, Some(key(11)));
    assert_eq!(c.quote_reserve, 5_000_000_000);
    let p = markets.get(&pool).unwrap();
    assert_eq!(p.venue, Some(Venue::PumpSwap));
    assert_eq!(p.pool_base_token_account, Some(key(24)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn swaps_accumulate_volume_across_slots() {
    let rig = start(8);
    let market = key(5);

    rig.engine
        .route(swap_event(
            1,
            100,
            swap(1, market, Venue::PumpFun, true, 1_000, 10),
        ))
        .await
        .unwrap();
    rig.engine
        .route(swap_event(
            2,
            101,
            swap(2, market, Venue::PumpFun, true, 2_000, 20),
        ))
        .await
        .unwrap();
    rig.engine
        .route(swap_event(
            3,
            101,
            swap(3, market, Venue::PumpFun, false, 500, 5),
        ))
        .await
        .unwrap();

    let markets = rig.markets().await;
    let metrics = rig.metrics.snapshot();
    rig.shutdown().await;

    let m = markets.get(&market).unwrap();
    assert_eq!(m.trade_count, 3);
    assert_eq!(m.buy_count, 2);
    assert_eq!(m.sell_count, 1);
    let totals = m.volume.totals();
    assert_eq!(totals.buy_quote, 3_000);
    assert_eq!(totals.sell_quote, 500);
    assert_eq!(totals.trades, 3);
    // Windowed query: only the trades in slot 101.
    assert_eq!(m.volume.totals_since(101).buy_quote, 2_000);
    assert_eq!(metrics.swaps_pumpfun, 3);
    assert_eq!(metrics.volume_updates, 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replayed_swap_transaction_is_not_double_counted() {
    let rig = start(8);
    let market = key(6);
    let ev = swap_event(9, 50, swap(9, market, Venue::PumpSwap, true, 1_234, 5_678));
    rig.engine.route(ev.clone()).await.unwrap();
    rig.engine.route(ev).await.unwrap();

    let markets = rig.markets().await;
    rig.shutdown().await;
    let m = markets.get(&market).unwrap();
    assert_eq!(m.trade_count, 1, "replayed tx must not double-count volume");
    assert_eq!(m.volume.totals().buy_quote, 1_234);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_swap_sequences_produce_identical_state() {
    let build = || {
        let market = key(7);
        let mut events = Vec::new();
        for step in 1..=50u8 {
            events.push(swap_event(
                step,
                200 + u64::from(step),
                swap(
                    step,
                    market,
                    Venue::PumpFun,
                    step % 2 == 0,
                    u64::from(step) * 100,
                    u64::from(step) * 1_000,
                ),
            ));
        }
        events
    };

    let run = |events: Vec<NormalizedEvent>| async move {
        let rig = start(8);
        for e in events {
            rig.engine.route(e).await.unwrap();
        }
        let markets = rig.markets().await;
        rig.shutdown().await;
        markets
    };

    let a = run(build()).await;
    let b = run(build()).await;
    assert_eq!(a, b, "same swap sequence must produce identical state");
    assert!(!a.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn created_markets_are_seeded_from_create_event() {
    let rig = start(8);
    let market = key(8);
    let created = CreatedMarket {
        venue: Venue::PumpFun,
        market_key: market,
        mint: key(88),
        quote_mint: None,
        virtual_base_reserve: Some(1_073_000_000_000_000),
        virtual_quote_reserve: Some(30_000_000_000),
        base_reserve: Some(793_100_000_000_000),
        token_total_supply: Some(1_000_000_000_000_000),
        timestamp: Some(1_700_000_100),
    };
    let mut signature = [0u8; 64];
    signature[0] = 42;
    rig.engine
        .route(NormalizedEvent::new(EventKind::Transaction(
            TransactionUpdate {
                signature,
                slot: 1,
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
            },
        )))
        .await
        .unwrap();

    let markets = rig.markets().await;
    rig.shutdown().await;
    let m = markets.get(&market).unwrap();
    assert_eq!(m.venue, Some(Venue::PumpFun));
    assert_eq!(m.base_mint, Some(key(88)));
    assert_eq!(m.virtual_quote_reserve, 30_000_000_000);
    assert_eq!(m.token_total_supply, 1_000_000_000_000_000);
}

#[test]
fn impossible_reserves_yield_no_price() {
    let mut m = MarketState::new(key(1), 0);
    assert!(m.spot_price_raw().is_none());
    assert!(m.executable_price(true, 100, 95).is_none());
    m.virtual_base_reserve = 1_000_000;
    m.virtual_quote_reserve = 0;
    m.quote_reserve = 0;
    assert!(m.spot_price_raw().is_none());
    m.virtual_quote_reserve = 30_000_000_000;
    assert!(m.spot_price_raw().is_some());
}
