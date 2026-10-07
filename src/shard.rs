//! A single market-state shard.
//!
//! Each shard is an independent task that *owns* its markets outright: there
//! is no shared map and no global lock. The only synchronisation is the
//! bounded channel that feeds the shard, which also preserves per-market
//! ordering because the router sends every event for a given market to the
//! same shard, in arrival order.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};

use crate::events::{EventKind, MarketKey, NormalizedEvent};
use crate::market::MarketState;
use crate::shutdown::Shutdown;
use crate::telemetry::Metrics;

/// Number of recent transaction signatures retained per shard for replay
/// protection. Bounded so memory cannot grow without limit.
const RECENT_SIGNATURE_CAPACITY: usize = 8_192;

/// Messages routed to a shard.
pub enum ShardMsg {
    /// One or more market keys affected by a single normalized event.
    ///
    /// A transaction can touch several markets in the same shard, so it is
    /// delivered as one message with a list of keys rather than N copies.
    Market {
        event: Arc<NormalizedEvent>,
        keys: Vec<MarketKey>,
    },
    /// A chain-global event (slot / block metadata) broadcast to all shards.
    Global { event: Arc<NormalizedEvent> },
    /// Read-only request for a consistent snapshot (tests / diagnostics).
    Snapshot(oneshot::Sender<ShardSnapshot>),
    /// Test-only concurrency probe: every shard must reach this barrier
    /// concurrently or the test deadlocks.
    #[cfg(test)]
    Barrier {
        barrier: Arc<tokio::sync::Barrier>,
        passed: Arc<std::sync::atomic::AtomicU64>,
    },
}

/// Diagnostic view of one shard.
#[derive(Debug, Clone)]
pub struct ShardSnapshot {
    pub id: usize,
    pub last_slot: u64,
    pub markets: Vec<MarketState>,
}

/// Bounded ring of recently seen transaction signatures.
struct SignatureRing {
    set: HashSet<[u8; 64]>,
    order: VecDeque<[u8; 64]>,
    capacity: usize,
}

impl SignatureRing {
    fn new(capacity: usize) -> Self {
        Self {
            set: HashSet::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Insert a signature. Returns `true` when it was new.
    fn insert(&mut self, sig: [u8; 64]) -> bool {
        if !self.set.insert(sig) {
            return false;
        }
        self.order.push_back(sig);
        if self.order.len() > self.capacity {
            if let Some(evicted) = self.order.pop_front() {
                self.set.remove(&evicted);
            }
        }
        true
    }
}

/// Shard-local state.
pub struct Shard {
    id: usize,
    markets: HashMap<MarketKey, MarketState>,
    recent_tx: SignatureRing,
    last_slot: u64,
    metrics: Arc<Metrics>,
}

impl Shard {
    pub fn new(id: usize, metrics: Arc<Metrics>) -> Self {
        Self {
            id,
            markets: HashMap::new(),
            recent_tx: SignatureRing::new(RECENT_SIGNATURE_CAPACITY),
            last_slot: 0,
            metrics,
        }
    }

    /// Run the shard until its channel closes or shutdown is requested.
    ///
    /// On shutdown the channel is drained rather than abandoned, so no
    /// already-routed event is lost and state remains consistent.
    pub async fn run(mut self, mut rx: mpsc::Receiver<ShardMsg>, shutdown: Shutdown) {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                msg = rx.recv() => match msg {
                    Some(msg) => {
                        self.metrics.observe_worker_thread(thread_hash());
                        self.handle(msg).await;
                    }
                    None => break,
                },
            }
        }
        // Drain anything already queued so shutdown does not corrupt state.
        while let Ok(msg) = rx.try_recv() {
            self.handle(msg).await;
        }
    }

    async fn handle(&mut self, msg: ShardMsg) {
        match msg {
            ShardMsg::Snapshot(tx) => {
                let mut markets: Vec<MarketState> = self.markets.values().cloned().collect();
                markets.sort_by_key(|m| m.key);
                let _ = tx.send(ShardSnapshot {
                    id: self.id,
                    last_slot: self.last_slot,
                    markets,
                });
            }
            ShardMsg::Global { event } => {
                self.last_slot = self.last_slot.max(event_slot(&event));
            }
            ShardMsg::Market { event, keys } => {
                // Replay protection is per event, not per key, so a transaction
                // that touches several markets in this shard is deduped once.
                if let EventKind::Transaction(t) = &event.kind {
                    if !self.recent_tx.insert(t.signature) {
                        self.metrics.incr_stale();
                        return;
                    }
                }
                let now = crate::clock::now_ns();
                let e2e_ns = (now.saturating_sub(event.arrival_ns)).min(u64::MAX as u128) as u64;
                let mut applied = 0u64;

                match &event.kind {
                    EventKind::Account(a) => {
                        let market = match self.markets.get_mut(&a.pubkey) {
                            Some(m) => m,
                            None => {
                                self.metrics.incr_active_market();
                                self.markets
                                    .entry(a.pubkey)
                                    .or_insert_with(|| MarketState::new(a.pubkey, now))
                            }
                        };
                        if a.decoded.is_some() {
                            self.metrics.incr_decoded();
                        }
                        let was_invalidated = market.invalidated_at_slot.is_some();
                        if market.apply_account(a, now) {
                            // A fresh authoritative account update at or after an
                            // invalidation re-establishes valid market state.
                            if was_invalidated && market.invalidated_at_slot.is_none() {
                                self.metrics.incr_reserve_revalidation();
                            }
                            applied += 1;
                        } else {
                            self.metrics.incr_stale();
                        }
                    }
                    EventKind::Transaction(t) => {
                        self.metrics.add_decode_rejected(t.decode_rejected);
                        // A failed (reverted) transaction is rolled back on
                        // chain, so its decoded contents must not mutate
                        // authoritative market state: no swaps, no creates, and
                        // no reserve-mutation invalidation. Observation (the
                        // touch below) still records that the market was
                        // involved.
                        if t.success {
                            // Non-trade reserve mutation (pump.fun fee sweep):
                            // invalidate every market this transaction touched
                            // so stale state cannot be quoted or executed
                            // against.
                            if t.has_reserve_mutation {
                                for key in &keys {
                                    if let Some(market) = self.markets.get_mut(key) {
                                        market.invalidate(t.slot);
                                        self.metrics.incr_reserve_invalidation();
                                    }
                                }
                            }
                            // Markets created by this transaction.
                            for created in &t.creates {
                                let market = self.market_mut(created.market_key, now);
                                market.note_slot(t.slot);
                                market.apply_create(created, now);
                                self.metrics.incr_decoded();
                            }
                            // Swaps: reserve/volume updates on the owning market.
                            for swap in &t.swaps {
                                let market = self.market_mut(swap.market_key, now);
                                market.apply_swap(swap, t.signature, t.slot, now);
                                self.metrics.incr_decoded();
                                self.metrics.incr_volume_update();
                                self.metrics.incr_swap(swap.venue);
                                applied += 1;
                            }
                        }
                        // Transaction observation on every touched market.
                        for key in keys {
                            if let Some(market) = self.markets.get_mut(&key) {
                                if market.apply_touch(t, now) {
                                    applied += 1;
                                }
                            }
                        }
                    }
                    EventKind::Slot(_) | EventKind::BlockMeta(_) => {}
                }
                if applied > 0 {
                    // Shard service time: dequeue -> applied.
                    let processing_ns =
                        (crate::clock::now_ns().saturating_sub(now)).min(u64::MAX as u128) as u64;
                    for _ in 0..applied {
                        self.metrics
                            .record_state_update(self.id, e2e_ns, processing_ns);
                    }
                }
            }
            #[cfg(test)]
            ShardMsg::Barrier { barrier, passed } => {
                barrier.wait().await;
                passed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }

    /// Get or create a market owned by this shard.
    fn market_mut(&mut self, key: MarketKey, now: u128) -> &mut MarketState {
        if !self.markets.contains_key(&key) {
            self.metrics.incr_active_market();
        }
        self.markets
            .entry(key)
            .or_insert_with(|| MarketState::new(key, now))
    }
}

fn event_slot(event: &NormalizedEvent) -> u64 {
    match &event.kind {
        EventKind::Slot(s) => s.slot,
        EventKind::BlockMeta(b) => b.slot,
        EventKind::Account(a) => a.slot,
        EventKind::Transaction(t) => t.slot,
    }
}

/// Stable, cheap hash of the current OS thread id, for parallelism evidence.
fn thread_hash() -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::thread::current().id().hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_ring_evicts_oldest() {
        let mut ring = SignatureRing::new(2);
        assert!(ring.insert([1u8; 64]));
        assert!(ring.insert([2u8; 64]));
        assert!(!ring.insert([1u8; 64]));
        assert!(ring.insert([3u8; 64])); // evicts [1]
        assert!(ring.insert([1u8; 64])); // now absent again
    }
}
