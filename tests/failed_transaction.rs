//! Failed (reverted) transactions must not mutate authoritative market state.
//!
//! On Solana a reverted transaction is rolled back, but its program logs — and
//! therefore any Anchor `Program data:` event decoded from them — can still be
//! present in the block metadata. The shard must not apply economic state
//! (swaps/creates) or reserve-mutation invalidation from such a transaction.
//!
//! Evidence level: integration over the real sharded engine.

use std::sync::Arc;

use neurone::decode::{CreatedMarket, DecodedAccount, DecodedSwap, Venue};
use neurone::engine::Engine;
use neurone::events::{AccountUpdate, EventKind, MarketKey, NormalizedEvent, TransactionUpdate};
use neurone::market::{MarketState, ReserveState};
use neurone::shutdown::{shutdown_channel, ShutdownHandle};
use neurone::telemetry::Metrics;

fn key(b: u8) -> MarketKey {
    let mut k = [0u8; 32];
    k[0] = b;
    k[31] = 0xF7;
    MarketKey(k)
}

/// A decoded bonding-curve account so `reserves_known` becomes true.
fn account_event(k: MarketKey, wv: u64, slot: u64) -> NormalizedEvent {
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

fn pf_swap(market: MarketKey, base_reserve: u64, quote_reserve: u64) -> DecodedSwap {
    DecodedSwap {
        venue: Venue::PumpFun,
        market_key: market,
        base_mint: Some(market),
        quote_mint: None,
        is_buy: true,
        base_amount: 10,
        quote_amount: 5,
        user_quote_amount: 5,
        base_reserve: Some(base_reserve),
        quote_reserve: Some(quote_reserve),
        virtual_base_reserve: Some(1_073_000_000_000_000),
        virtual_quote_reserve: Some(30_000_000_000),
        fee_quote: 0,
        fee_bps: Some(95),
        timestamp: Some(1_700_000_000),
        ix_name: None,
    }
}

fn created(market: MarketKey) -> CreatedMarket {
    CreatedMarket {
        venue: Venue::PumpFun,
        market_key: market,
        mint: key(0x11),
        quote_mint: None,
        virtual_base_reserve: Some(1_073_000_000_000_000),
        virtual_quote_reserve: Some(30_000_000_000),
        base_reserve: Some(793_100_000_000_000),
        token_total_supply: Some(1_000_000_000_000_000),
        timestamp: Some(1_700_000_100),
    }
}

#[allow(clippy::too_many_arguments)]
fn tx_event(
    sig: u8,
    slot: u64,
    keys: Vec<MarketKey>,
    swaps: Vec<DecodedSwap>,
    creates: Vec<CreatedMarket>,
    has_reserve_mutation: bool,
    success: bool,
) -> NormalizedEvent {
    let mut signature = [0u8; 64];
    signature[0] = sig;
    NormalizedEvent::new(EventKind::Transaction(TransactionUpdate {
        signature,
        slot,
        index: 0,
        is_vote: false,
        success,
        keys,
        swaps,
        creates,
        decode_rejected: 0,
        vault_balances: Vec::new(),
        has_reserve_mutation,
        has_sweep: false,
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

/// 1: a successful transaction still applies its decoded swap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn successful_transaction_applies_swap() {
    let rig = start(8);
    let m = key(1);
    rig.engine.route(account_event(m, 1, 100)).await.unwrap();
    rig.engine
        .route(tx_event(
            1,
            101,
            vec![m],
            vec![pf_swap(m, 900_000_000, 6_000_000_000)],
            Vec::new(),
            false,
            true,
        ))
        .await
        .unwrap();

    let state = rig.market(m).await.expect("market present");
    assert_eq!(state.trade_count, 1);
    assert_eq!(state.base_reserve, 900_000_000);
    assert_eq!(state.quote_reserve, 6_000_000_000);
    rig.shutdown().await;
}

/// 2: a failed transaction does not apply its decoded swap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_transaction_does_not_apply_swap() {
    let rig = start(8);
    let m = key(2);
    rig.engine.route(account_event(m, 1, 100)).await.unwrap();
    rig.engine
        .route(tx_event(
            2,
            101,
            vec![m],
            vec![pf_swap(m, 900_000_000, 6_000_000_000)],
            Vec::new(),
            false,
            false,
        ))
        .await
        .unwrap();

    let state = rig.market(m).await.expect("market present");
    assert_eq!(state.trade_count, 0, "failed swap must not be applied");
    assert_eq!(
        state.base_reserve, 800_000_000,
        "reserves must be unchanged"
    );
    assert_eq!(state.quote_reserve, 5_000_000_000);
    rig.shutdown().await;
}

/// 3: a failed transaction does not create a market from a decoded create, while
/// the equivalent successful transaction does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_transaction_does_not_create_market() {
    let rig = start(8);
    let failed = key(3);
    let ok = key(4);
    rig.engine
        .route(tx_event(
            3,
            50,
            vec![failed],
            Vec::new(),
            vec![created(failed)],
            false,
            false,
        ))
        .await
        .unwrap();
    rig.engine
        .route(tx_event(
            4,
            51,
            vec![ok],
            Vec::new(),
            vec![created(ok)],
            false,
            true,
        ))
        .await
        .unwrap();

    assert!(
        rig.market(failed).await.is_none(),
        "failed create must not make a market"
    );
    assert!(
        rig.market(ok).await.is_some(),
        "successful create must make a market"
    );
    rig.shutdown().await;
}

/// 4: a failed reserve-mutation transaction does not invalidate state, while the
/// successful equivalent does.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_reserve_mutation_does_not_invalidate() {
    let rig = start(8);
    let m = key(5);
    rig.engine.route(account_event(m, 1, 100)).await.unwrap();
    // Failed sweep: state must stay Known.
    rig.engine
        .route(tx_event(
            5,
            200,
            vec![m],
            Vec::new(),
            Vec::new(),
            true,
            false,
        ))
        .await
        .unwrap();
    let state = rig.market(m).await.expect("market present");
    assert_eq!(state.reserve_state(200, 150), ReserveState::Known);
    assert!(state.invalidated_at_slot.is_none());

    // Successful sweep on a different market does invalidate it.
    let m2 = key(6);
    rig.engine.route(account_event(m2, 1, 100)).await.unwrap();
    rig.engine
        .route(tx_event(
            6,
            200,
            vec![m2],
            Vec::new(),
            Vec::new(),
            true,
            true,
        ))
        .await
        .unwrap();
    assert_eq!(
        rig.market(m2).await.unwrap().reserve_state(200, 150),
        ReserveState::Invalidated
    );
    rig.shutdown().await;
}
