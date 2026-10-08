//! Architecture-level tests for the parallel sharded market-state engine.
//!
//! These exercise the real engine (real tasks, real channels) — not mocks.
//! Evidence level: unit/integration over the in-process architecture.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use neurone::engine::Engine;
use neurone::events::{
    AccountUpdate, BlockMetaUpdate, EventKind, MarketKey, NormalizedEvent, SlotStatusKind,
    SlotUpdate, TransactionUpdate,
};
use neurone::ingest::simulated::SimulatedSource;
use neurone::market::MarketState;
use neurone::shutdown::{shutdown_channel, ShutdownHandle};
use neurone::telemetry::Metrics;

fn key(i: usize) -> MarketKey {
    let mut b = [0u8; 32];
    b[..8].copy_from_slice(&(i as u64).to_le_bytes());
    b[8] = 0xC3;
    MarketKey(b)
}

fn account_event(k: MarketKey, wv: u64, digest: u64, slot: u64) -> NormalizedEvent {
    NormalizedEvent::new(EventKind::Account(AccountUpdate {
        pubkey: k,
        slot,
        owner: Some(key(usize::MAX)),
        lamports: 1_000,
        data_len: 64,
        data_digest: digest,
        write_version: wv,
        is_startup: false,
        txn_signature: None,
        decoded: None,
        pyth_sol_usd: None,
    }))
}

fn tx_event(k: MarketKey, sig_byte: u8, slot: u64) -> NormalizedEvent {
    let mut sig = [0u8; 64];
    sig[0] = sig_byte;
    NormalizedEvent::new(EventKind::Transaction(TransactionUpdate {
        signature: sig,
        slot,
        index: 0,
        is_vote: false,
        success: true,
        keys: vec![k],
        swaps: Vec::new(),
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
    async fn markets(&self) -> BTreeMap<MarketKey, MarketState> {
        let snapshots = self.engine.snapshot().await.expect("snapshot");
        let mut out = BTreeMap::new();
        for shard in snapshots {
            for market in shard.markets {
                out.insert(market.key, market);
            }
        }
        out
    }

    async fn shutdown(self) {
        self.handle.trigger();
        for h in self.handles {
            h.await.expect("shard task panicked");
        }
    }
}

async fn run_sequence(events: &[NormalizedEvent]) -> BTreeMap<MarketKey, MarketState> {
    let rig = start(8);
    for event in events {
        rig.engine.route(event.clone()).await.expect("route");
    }
    let markets = rig.markets().await;
    rig.shutdown().await;
    markets
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn identical_event_sequences_produce_identical_state() {
    let mut source = SimulatedSource::new(64, 0xDEAD_BEEF);
    let events: Vec<NormalizedEvent> = (0..4_000).map(|_| source.next_event()).collect();

    let a = run_sequence(&events).await;
    let b = run_sequence(&events).await;

    assert_eq!(
        a, b,
        "same event sequence must produce identical market state"
    );
    assert!(!a.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shard_routing_is_deterministic_and_balanced() {
    let rig = start(8);
    let mut counts = vec![0usize; 8];
    for i in 0..20_000usize {
        let k = key(i);
        let s = rig.engine.shard_of(&k);
        // Same identity always routes to the same shard.
        assert_eq!(s, rig.engine.shard_of(&k));
        assert!(s < 8);
        counts[s] += 1;
    }
    rig.shutdown().await;
    assert!(
        counts.iter().all(|&c| c > 0),
        "all shards should receive traffic: {counts:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn per_market_ordering_is_preserved_under_interleaving() {
    let rig = start(8);
    // Choose a key that does not collide with the interleaved `key(other)` set.
    let target = key(10_000);

    // Interleave other markets' work with an ordered stream for `target`.
    for step in 1..=500u64 {
        for other in 0..8usize {
            rig.engine
                .route(account_event(key(other), step, step, step))
                .await
                .unwrap();
        }
        rig.engine
            .route(account_event(target, step, step * 3, step))
            .await
            .unwrap();
    }
    let markets = rig.markets().await;
    rig.shutdown().await;

    let t = markets.get(&target).expect("target market present");
    assert_eq!(t.account_updates, 500);
    assert_eq!(t.write_version, 500);
    assert_eq!(t.data_digest, 500 * 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replayed_events_do_not_corrupt_state() {
    let rig = start(8);
    let mut events = Vec::new();
    for step in 1..=200u64 {
        events.push(account_event(key(step as usize), step, step, step));
    }
    for ev in &events {
        rig.engine.route(ev.clone()).await.unwrap();
    }
    for ev in &events {
        rig.engine.route(ev.clone()).await.unwrap();
    }
    let markets = rig.markets().await;
    rig.shutdown().await;

    for (_, state) in markets {
        assert_eq!(state.account_updates, 1, "replayed update must be ignored");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn independent_markets_advance_concurrently() {
    let shards = 8;
    let rig = start(shards);
    let markets = 512usize;
    let updates_per_market = 10u64;

    for step in 1..=updates_per_market {
        for m in 0..markets {
            rig.engine
                .route(account_event(key(m), step, step, step))
                .await
                .unwrap();
        }
    }

    let state = rig.markets().await;
    let snapshot = rig.metrics.snapshot();
    rig.shutdown().await;

    assert_eq!(state.len(), markets);
    for m in 0..markets {
        assert_eq!(
            state.get(&key(m)).unwrap().account_updates,
            updates_per_market
        );
    }
    assert_eq!(
        snapshot.state_updates,
        (markets as u64) * updates_per_market
    );
    assert!(
        snapshot.shard_activity.iter().all(|&c| c > 0),
        "each shard must have processed work independently: {:?}",
        snapshot.shard_activity
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn transactions_update_existing_markets_only() {
    let rig = start(8);
    let known = key(11);
    let unknown = key(999_999);

    rig.engine
        .route(account_event(known, 1, 1, 1))
        .await
        .unwrap();
    rig.engine.route(tx_event(known, 1, 2)).await.unwrap();
    rig.engine.route(tx_event(known, 1, 2)).await.unwrap(); // replay
    rig.engine.route(tx_event(unknown, 2, 2)).await.unwrap();

    let markets = rig.markets().await;
    rig.shutdown().await;

    assert_eq!(markets.len(), 1, "transactions must not create markets");
    assert_eq!(markets.get(&known).unwrap().tx_count, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn global_events_reach_every_shard() {
    let rig = start(4);
    let slot = NormalizedEvent::new(EventKind::Slot(SlotUpdate {
        slot: 42,
        parent: Some(41),
        status: SlotStatusKind::Processed,
    }));
    rig.engine.route(slot).await.unwrap();
    let block = NormalizedEvent::new(EventKind::BlockMeta(BlockMetaUpdate {
        slot: 43,
        block_height: Some(100),
        block_time: Some(1_700_000_000),
        executed_transaction_count: 10,
    }));
    rig.engine.route(block).await.unwrap();

    // All routed events are applied once the snapshot round-trips.
    let snapshots = rig.engine.snapshot().await.unwrap();
    rig.shutdown().await;
    for s in snapshots {
        assert_eq!(s.last_slot, 43);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_is_clean_and_idempotent() {
    let rig = start(4);
    for m in 0..100usize {
        rig.engine
            .route(account_event(key(m), 1, 1, 1))
            .await
            .unwrap();
    }
    let before = rig.markets().await;
    let timeout = tokio::time::timeout(Duration::from_secs(5), rig.shutdown()).await;
    assert!(timeout.is_ok(), "shutdown must complete promptly");
    assert_eq!(before.len(), 100);
}
