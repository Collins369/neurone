//! Throughput / latency harness.
//!
//! Correctness first, then measurement. This harness drives the real engine
//! with the deterministic synthetic source so events/sec, latency and shard
//! distribution can be measured without a live credential.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::engine::Engine;
use crate::error::Result;
use crate::ingest::simulated::SimulatedSource;
use crate::shutdown::shutdown_channel;
use crate::telemetry::{Metrics, MetricsSnapshot};

/// Result of a benchmark run.
#[derive(Debug, Clone)]
pub struct BenchReport {
    pub events: u64,
    pub elapsed: Duration,
    pub events_per_second: f64,
    pub worker_threads: u32,
    pub metrics: MetricsSnapshot,
    pub latency_buckets: (Vec<u64>, Vec<u64>),
}

/// Drive `events` synthetic events through the engine and report throughput.
///
/// When `rate_per_sec` is `Some`, the producer is paced so the shards keep up
/// and the end-to-end latency reflects steady-state service time rather than
/// an artificial burst backlog. When `None`, the producer runs flat out to
/// measure peak throughput.
pub async fn run(config: &Config, events: u64, rate_per_sec: Option<u64>) -> Result<BenchReport> {
    let metrics = Metrics::new(
        config.runtime.shards,
        config.telemetry.latency_boundaries_ns.clone(),
    );
    let (handle, shutdown) = shutdown_channel();
    let (engine, shard_handles) = Engine::start(
        config.runtime.shards,
        config.runtime.shard_channel_capacity,
        Arc::clone(&metrics),
        shutdown,
    );
    let mut source = SimulatedSource::new(config.ingest.simulated.markets, 0x0042_454E_4348);

    let mut ticker = rate_per_sec.map(|rate| {
        let mut t = tokio::time::interval(Duration::from_nanos(1_000_000_000 / rate.max(1)));
        t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
        t
    });

    let start = Instant::now();
    for _ in 0..events {
        if let Some(t) = ticker.as_mut() {
            t.tick().await;
        }
        engine.route(source.next_event()).await?;
    }
    // A snapshot round-trips the whole pipeline, so it doubles as a drain
    // barrier: when it returns, every previously routed event is applied.
    let snapshots = engine.snapshot().await?;
    let elapsed = start.elapsed();

    let market_total: usize = snapshots.iter().map(|s| s.markets.len()).sum();
    let snapshot = metrics.snapshot();
    debug_assert_eq!(market_total as u64, snapshot.active_market_states);

    handle.trigger();
    for handle in shard_handles {
        let _ = handle.await;
    }

    let secs = elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
    Ok(BenchReport {
        events,
        elapsed,
        events_per_second: events as f64 / secs,
        worker_threads: metrics.observed_worker_threads(),
        metrics: snapshot,
        latency_buckets: metrics.latency_buckets(),
    })
}
