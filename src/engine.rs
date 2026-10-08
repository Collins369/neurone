//! The parallel sharded market-state engine.
//!
//! ```text
//! normalized event
//!     -> hash(market key)
//!     -> shard
//!     -> owned market state
//! ```
//!
//! The engine is the only component that knows the shard layout. Callers route
//! whole events; the engine fans them out to the owning shard(s). Independent
//! markets live in different shards and therefore advance concurrently.

use std::sync::Arc;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::error::{Error, Result};
use crate::events::{EventKind, MarketKey, NormalizedEvent};
use crate::hash::shard_for;
use crate::reference::SolUsdReferenceState;
use crate::shard::{Shard, ShardMsg, ShardSnapshot};
use crate::shutdown::Shutdown;
use crate::strategy::StrategyConfig;
use crate::telemetry::Metrics;

/// Routing handle + owned shard tasks.
#[derive(Clone)]
pub struct Engine {
    senders: Vec<mpsc::Sender<ShardMsg>>,
    num_shards: usize,
    metrics: Arc<Metrics>,
    sol_usd: Arc<SolUsdReferenceState>,
}

impl Engine {
    /// Spawn `num_shards` independent shard tasks.
    pub fn start(
        num_shards: usize,
        channel_capacity: usize,
        metrics: Arc<Metrics>,
        shutdown: Shutdown,
    ) -> (Engine, Vec<JoinHandle<()>>) {
        Self::start_with_strategy(
            num_shards,
            channel_capacity,
            metrics,
            shutdown,
            Arc::new(StrategyConfig::default()),
            crate::config::MarketConfig::default().reserve_stale_slots,
        )
    }

    /// Spawn `num_shards` with an explicit M4 strategy configuration.
    pub fn start_with_strategy(
        num_shards: usize,
        channel_capacity: usize,
        metrics: Arc<Metrics>,
        shutdown: Shutdown,
        strategy: Arc<StrategyConfig>,
        reserve_stale_slots: u64,
    ) -> (Engine, Vec<JoinHandle<()>>) {
        assert!(num_shards >= 1, "engine requires at least one shard");
        let sol_usd = Arc::new(SolUsdReferenceState::new());
        let mut senders = Vec::with_capacity(num_shards);
        let mut handles = Vec::with_capacity(num_shards);
        for id in 0..num_shards {
            let (tx, rx) = mpsc::channel(channel_capacity);
            let shard = Shard::new(
                id,
                Arc::clone(&metrics),
                Arc::clone(&strategy),
                reserve_stale_slots,
                Arc::clone(&sol_usd),
            );
            let shutdown = shutdown.clone();
            handles.push(tokio::spawn(async move { shard.run(rx, shutdown).await }));
            senders.push(tx);
        }
        (
            Engine {
                senders,
                num_shards,
                metrics,
                sol_usd,
            },
            handles,
        )
    }

    pub fn num_shards(&self) -> usize {
        self.num_shards
    }

    /// Shared SOL/USD reference state (Pyth over Yellowstone).
    pub fn sol_usd(&self) -> Arc<SolUsdReferenceState> {
        Arc::clone(&self.sol_usd)
    }

    /// Deterministic shard for a market identity.
    pub fn shard_of(&self, key: &MarketKey) -> usize {
        shard_for(key.as_bytes(), self.num_shards)
    }

    /// Route one normalized event to its owning shard(s).
    ///
    /// Ordering: the router is a single task and every event for a given
    /// market key is sent, in arrival order, to that key's shard channel. Per
    /// market, the shard therefore observes events in exactly the order they
    /// were routed. Different markets in different shards proceed in parallel.
    pub async fn route(&self, event: NormalizedEvent) -> Result<()> {
        let event = Arc::new(event);
        match &event.kind {
            // Chain-global events go to every shard so each local state machine
            // can maintain its own slot watermark.
            EventKind::Slot(_) | EventKind::BlockMeta(_) => {
                for tx in &self.senders {
                    self.send(
                        tx,
                        ShardMsg::Global {
                            event: Arc::clone(&event),
                        },
                    )
                    .await?;
                }
            }
            EventKind::Account(a) => {
                // The Pyth SOL/USD price account feeds the reference state, not
                // a market shard: no fake market, no shard routing.
                if let Some(u) = &a.pyth_sol_usd {
                    if self.sol_usd.update(u, crate::clock::now_ns() as u64) {
                        self.metrics.incr_sol_usd_reference();
                    }
                    return Ok(());
                }
                let shard = self.shard_of(&a.pubkey);
                self.send(
                    &self.senders[shard],
                    ShardMsg::Market {
                        event: Arc::clone(&event),
                        keys: vec![a.pubkey],
                    },
                )
                .await?;
            }
            EventKind::Transaction(t) => {
                // Group the touched keys by shard so each shard receives one
                // message with only the keys it owns.
                let mut groups: Vec<Vec<MarketKey>> = vec![Vec::new(); self.num_shards];
                for key in &t.keys {
                    groups[self.shard_of(key)].push(*key);
                }
                for (shard, keys) in groups.into_iter().enumerate() {
                    if keys.is_empty() {
                        continue;
                    }
                    self.send(
                        &self.senders[shard],
                        ShardMsg::Market {
                            event: Arc::clone(&event),
                            keys,
                        },
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }

    async fn send(&self, tx: &mpsc::Sender<ShardMsg>, msg: ShardMsg) -> Result<()> {
        tx.send(msg).await.map_err(|_| {
            self.metrics.incr_errors();
            Error::Routing("shard channel closed".into())
        })
    }

    /// Collect a snapshot of every shard's state (tests / diagnostics).
    pub async fn snapshot(&self) -> Result<Vec<ShardSnapshot>> {
        let mut receivers = Vec::with_capacity(self.num_shards);
        for tx in &self.senders {
            let (snap_tx, snap_rx) = oneshot::channel();
            tx.send(ShardMsg::Snapshot(snap_tx))
                .await
                .map_err(|_| Error::Routing("shard channel closed".into()))?;
            receivers.push(snap_rx);
        }
        let mut out = Vec::with_capacity(self.num_shards);
        for rx in receivers {
            out.push(
                rx.await
                    .map_err(|_| Error::Routing("shard dropped before snapshot".into()))?,
            );
        }
        out.sort_by_key(|s| s.id);
        Ok(out)
    }

    /// Test-only: make every shard rendezvous on a barrier simultaneously.
    ///
    /// If shards did not run as independent tasks, the first shard would block
    /// the single worker forever and this call would never complete.
    #[cfg(test)]
    pub async fn rendezvous_all(&self, shards: usize) -> Result<Arc<std::sync::atomic::AtomicU64>> {
        let barrier = Arc::new(tokio::sync::Barrier::new(shards));
        let passed = Arc::new(std::sync::atomic::AtomicU64::new(0));
        for tx in &self.senders {
            self.send(
                tx,
                ShardMsg::Barrier {
                    barrier: Arc::clone(&barrier),
                    passed: Arc::clone(&passed),
                },
            )
            .await?;
        }
        Ok(passed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    /// Proof of architecture: the shards are genuinely independent tasks.
    ///
    /// Every shard is asked to rendezvous on a barrier sized to the shard
    /// count. A serial implementation (one task draining all shards) would
    /// block on the first shard's `wait()` and never reach the others, so the
    /// barrier would never release. Independent tasks all arrive and pass.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shards_run_as_independent_tasks() {
        let (handle, shutdown) = crate::shutdown::shutdown_channel();
        let metrics = crate::telemetry::Metrics::new(4, vec![1_000]);
        let (engine, handles) = Engine::start(4, 8, metrics, shutdown);

        let passed = engine.rendezvous_all(4).await.expect("send barrier");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while passed.load(Ordering::SeqCst) < 4 {
            assert!(
                tokio::time::Instant::now() < deadline,
                "shards did not run concurrently: barrier never released"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        handle.trigger();
        for h in handles {
            h.await.expect("shard task panicked");
        }
    }
}
