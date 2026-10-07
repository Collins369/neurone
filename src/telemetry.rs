//! Non-blocking operational telemetry.
//!
//! Every counter is a plain atomic and every latency sample lands in a
//! fixed-bucket atomic histogram. There is no channel, mutex or I/O in the
//! recording path, so telemetry can never stall event processing. A separate
//! reporter task renders periodic snapshots.
//!
//! Two latency distributions are tracked because they answer different
//! questions:
//! * **end-to-end** (`arrival -> state update`) includes time spent queued for
//!   a shard; it is what a consumer of the stream actually experiences.
//! * **processing** (`dequeue -> applied`) is the shard's own hot-path cost,
//!   independent of backlog.
//!
//! The blueprint's later stages (trigger/submit/land) extend the end-to-end
//! timeline; the recording API here is deliberately narrow so those stages can
//! be added without touching the hot path.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::shutdown::Shutdown;

/// Fixed-bucket latency histogram. Bucket `i` counts samples `<= bounds[i]`;
/// the final bucket counts everything above the last bound.
struct Histogram {
    bounds: Vec<u64>,
    buckets: Vec<AtomicU64>,
}

impl Histogram {
    fn new(bounds: Vec<u64>) -> Self {
        let buckets = (0..=bounds.len()).map(|_| AtomicU64::new(0)).collect();
        Self { bounds, buckets }
    }

    #[inline]
    fn record(&self, ns: u64) {
        let idx = self
            .bounds
            .iter()
            .position(|&b| ns <= b)
            .unwrap_or(self.bounds.len());
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    fn counts(&self) -> Vec<u64> {
        self.buckets
            .iter()
            .map(|a| a.load(Ordering::Relaxed))
            .collect()
    }

    fn quantile(&self, q: f64) -> u64 {
        let counts = self.counts();
        let total: u64 = counts.iter().sum();
        if total == 0 {
            return 0;
        }
        let target = (total as f64 * q).ceil() as u64;
        let mut cumulative = 0u64;
        for (i, &c) in counts.iter().enumerate() {
            cumulative += c;
            if cumulative >= target {
                return self.bounds.get(i).copied().unwrap_or_else(|| {
                    // Overflow bucket: report the last boundary (a lower bound
                    // on the true max), sufficient for percentile triage.
                    self.bounds.last().copied().unwrap_or(0)
                });
            }
        }
        self.bounds.last().copied().unwrap_or(0)
    }

    fn samples(&self) -> u64 {
        self.buckets.iter().map(|a| a.load(Ordering::Relaxed)).sum()
    }
}

/// Lock-free metrics collector shared by ingestion, shards and the reporter.
pub struct Metrics {
    pub(crate) yellowstone_connected: AtomicU64,
    pub(crate) events_received: AtomicU64,
    pub(crate) events_normalized: AtomicU64,
    pub(crate) events_decoded: AtomicU64,
    pub(crate) decode_rejected: AtomicU64,
    pub(crate) volume_updates: AtomicU64,
    pub(crate) stale_events: AtomicU64,
    pub(crate) reserve_invalidations: AtomicU64,
    pub(crate) reserve_revalidations: AtomicU64,
    pub(crate) swaps_pumpfun: AtomicU64,
    pub(crate) swaps_pumpswap: AtomicU64,
    pub(crate) invalid_events: AtomicU64,
    pub(crate) state_updates: AtomicU64,
    pub(crate) slots_observed: AtomicU64,
    pub(crate) latest_slot: AtomicU64,
    pub(crate) reconnects: AtomicU64,
    pub(crate) errors: AtomicU64,
    pub(crate) active_market_states: AtomicU64,
    pub(crate) worker_threads_mask: AtomicU64,
    shard_activity: Vec<AtomicU64>,
    end_to_end: Histogram,
    processing: Histogram,
    decode: Histogram,
}

/// Immutable point-in-time view of [`Metrics`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsSnapshot {
    pub yellowstone_connected: bool,
    pub events_received: u64,
    pub events_normalized: u64,
    pub events_decoded: u64,
    pub decode_rejected: u64,
    pub volume_updates: u64,
    pub stale_events: u64,
    pub reserve_invalidations: u64,
    pub reserve_revalidations: u64,
    pub swaps_pumpfun: u64,
    pub swaps_pumpswap: u64,
    pub invalid_events: u64,
    pub state_updates: u64,
    pub slots_observed: u64,
    pub latest_slot: u64,
    pub reconnects: u64,
    pub errors: u64,
    pub active_market_states: u64,
    pub worker_threads: u32,
    pub shard_activity: Vec<u64>,
    /// Arrival -> state update (includes shard queueing).
    pub latency_samples: u64,
    pub latency_p50_ns: u64,
    pub latency_p95_ns: u64,
    pub latency_p99_ns: u64,
    pub latency_max_ns: u64,
    /// Shard dequeue -> applied (hot-path service time).
    pub processing_samples: u64,
    pub processing_p50_ns: u64,
    pub processing_p95_ns: u64,
    pub processing_p99_ns: u64,
    pub processing_max_ns: u64,
    /// Normalize + protocol decode time at the ingestion boundary.
    pub decode_samples: u64,
    pub decode_p50_ns: u64,
    pub decode_p95_ns: u64,
    pub decode_p99_ns: u64,
    pub decode_max_ns: u64,
}

impl Metrics {
    /// Build a collector with `num_shards` per-shard activity counters.
    pub fn new(num_shards: usize, latency_bounds: Vec<u64>) -> Arc<Self> {
        assert!(
            !latency_bounds.is_empty(),
            "latency bounds must not be empty"
        );
        let shard_activity = (0..num_shards).map(|_| AtomicU64::new(0)).collect();
        Arc::new(Self {
            yellowstone_connected: AtomicU64::new(0),
            events_received: AtomicU64::new(0),
            events_normalized: AtomicU64::new(0),
            events_decoded: AtomicU64::new(0),
            decode_rejected: AtomicU64::new(0),
            volume_updates: AtomicU64::new(0),
            stale_events: AtomicU64::new(0),
            reserve_invalidations: AtomicU64::new(0),
            reserve_revalidations: AtomicU64::new(0),
            swaps_pumpfun: AtomicU64::new(0),
            swaps_pumpswap: AtomicU64::new(0),
            invalid_events: AtomicU64::new(0),
            state_updates: AtomicU64::new(0),
            slots_observed: AtomicU64::new(0),
            latest_slot: AtomicU64::new(0),
            reconnects: AtomicU64::new(0),
            errors: AtomicU64::new(0),
            active_market_states: AtomicU64::new(0),
            worker_threads_mask: AtomicU64::new(0),
            shard_activity,
            end_to_end: Histogram::new(latency_bounds.clone()),
            processing: Histogram::new(latency_bounds.clone()),
            decode: Histogram::new(latency_bounds),
        })
    }

    pub fn set_connected(&self, connected: bool) {
        self.yellowstone_connected
            .store(u64::from(connected), Ordering::Relaxed);
    }

    pub fn incr_received(&self) {
        self.events_received.fetch_add(1, Ordering::Relaxed);
    }

    pub fn incr_normalized(&self) {
        self.events_normalized.fetch_add(1, Ordering::Relaxed);
    }

    pub fn incr_decoded(&self) {
        self.events_decoded.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add_decode_rejected(&self, n: u32) {
        self.decode_rejected
            .fetch_add(u64::from(n), Ordering::Relaxed);
    }

    pub fn incr_volume_update(&self) {
        self.volume_updates.fetch_add(1, Ordering::Relaxed);
    }

    pub fn incr_swap(&self, venue: crate::decode::Venue) {
        match venue {
            crate::decode::Venue::PumpFun => self.swaps_pumpfun.fetch_add(1, Ordering::Relaxed),
            crate::decode::Venue::PumpSwap => self.swaps_pumpswap.fetch_add(1, Ordering::Relaxed),
        };
    }

    pub fn incr_stale(&self) {
        self.stale_events.fetch_add(1, Ordering::Relaxed);
    }

    /// Count a market-state invalidation caused by a non-trade reserve
    /// mutation (e.g. a pump.fun fee sweep).
    pub fn incr_reserve_invalidation(&self) {
        self.reserve_invalidations.fetch_add(1, Ordering::Relaxed);
    }

    /// Count a market whose invalidated state was re-established by a fresh
    /// authoritative account update (the M3.3 recovery path). This makes the
    /// invalidation lifecycle observable end to end.
    pub fn incr_reserve_revalidation(&self) {
        self.reserve_revalidations.fetch_add(1, Ordering::Relaxed);
    }

    /// Record the normalize+decode time for one update.
    pub fn record_decode(&self, ns: u64) {
        self.decode.record(ns);
    }

    pub fn incr_invalid(&self) {
        self.invalid_events.fetch_add(1, Ordering::Relaxed);
    }

    pub fn incr_errors(&self) {
        self.errors.fetch_add(1, Ordering::Relaxed);
    }

    pub fn incr_reconnects(&self) {
        self.reconnects.fetch_add(1, Ordering::Relaxed);
    }

    pub fn incr_active_market(&self) {
        self.active_market_states.fetch_add(1, Ordering::Relaxed);
    }

    /// Track the highest slot seen in any update.
    pub fn observe_latest_slot(&self, slot: u64) {
        self.latest_slot.fetch_max(slot, Ordering::Relaxed);
    }

    /// Count one observed slot update.
    pub fn observe_slot(&self) {
        self.slots_observed.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one successfully applied state mutation with both latencies.
    pub fn record_state_update(&self, shard: usize, e2e_ns: u64, processing_ns: u64) {
        self.state_updates.fetch_add(1, Ordering::Relaxed);
        if let Some(counter) = self.shard_activity.get(shard) {
            counter.fetch_add(1, Ordering::Relaxed);
        }
        self.end_to_end.record(e2e_ns);
        self.processing.record(processing_ns);
    }

    /// Note that this OS thread executed event work (parallelism evidence).
    pub fn observe_worker_thread(&self, thread_id_hash: u64) {
        let bit = 1u64 << (thread_id_hash % 64);
        self.worker_threads_mask.fetch_or(bit, Ordering::Relaxed);
    }

    /// Number of distinct worker-thread buckets that executed event work.
    pub fn observed_worker_threads(&self) -> u32 {
        self.worker_threads_mask
            .load(Ordering::Relaxed)
            .count_ones()
    }

    /// Take a snapshot for reporting and tests.
    pub fn snapshot(&self) -> MetricsSnapshot {
        let activity: Vec<u64> = self
            .shard_activity
            .iter()
            .map(|a| a.load(Ordering::Relaxed))
            .collect();
        MetricsSnapshot {
            yellowstone_connected: self.yellowstone_connected.load(Ordering::Relaxed) != 0,
            events_received: self.events_received.load(Ordering::Relaxed),
            events_normalized: self.events_normalized.load(Ordering::Relaxed),
            events_decoded: self.events_decoded.load(Ordering::Relaxed),
            decode_rejected: self.decode_rejected.load(Ordering::Relaxed),
            volume_updates: self.volume_updates.load(Ordering::Relaxed),
            stale_events: self.stale_events.load(Ordering::Relaxed),
            reserve_invalidations: self.reserve_invalidations.load(Ordering::Relaxed),
            reserve_revalidations: self.reserve_revalidations.load(Ordering::Relaxed),
            swaps_pumpfun: self.swaps_pumpfun.load(Ordering::Relaxed),
            swaps_pumpswap: self.swaps_pumpswap.load(Ordering::Relaxed),
            invalid_events: self.invalid_events.load(Ordering::Relaxed),
            state_updates: self.state_updates.load(Ordering::Relaxed),
            slots_observed: self.slots_observed.load(Ordering::Relaxed),
            latest_slot: self.latest_slot.load(Ordering::Relaxed),
            reconnects: self.reconnects.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
            active_market_states: self.active_market_states.load(Ordering::Relaxed),
            worker_threads: self.observed_worker_threads(),
            shard_activity: activity,
            latency_samples: self.end_to_end.samples(),
            latency_p50_ns: self.end_to_end.quantile(0.50),
            latency_p95_ns: self.end_to_end.quantile(0.95),
            latency_p99_ns: self.end_to_end.quantile(0.99),
            latency_max_ns: self.end_to_end.quantile(1.0),
            processing_samples: self.processing.samples(),
            processing_p50_ns: self.processing.quantile(0.50),
            processing_p95_ns: self.processing.quantile(0.95),
            processing_p99_ns: self.processing.quantile(0.99),
            processing_max_ns: self.processing.quantile(1.0),
            decode_samples: self.decode.samples(),
            decode_p50_ns: self.decode.quantile(0.50),
            decode_p95_ns: self.decode.quantile(0.95),
            decode_p99_ns: self.decode.quantile(0.99),
            decode_max_ns: self.decode.quantile(1.0),
        }
    }

    /// Raw end-to-end histogram (bucket upper bounds in ns + counts).
    pub fn latency_buckets(&self) -> (Vec<u64>, Vec<u64>) {
        (self.end_to_end.bounds.clone(), self.end_to_end.counts())
    }
}

/// Render periodic snapshots at `interval` until shutdown.
pub async fn report_loop(metrics: Arc<Metrics>, interval: Duration, shutdown: Shutdown) {
    let mut previous = metrics.snapshot();
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await; // consume the immediate first tick
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = ticker.tick() => {
                let now = metrics.snapshot();
                let delta_events = now.events_received.saturating_sub(previous.events_received);
                let delta_updates = now.state_updates.saturating_sub(previous.state_updates);
                let secs = interval.as_secs_f64();
                tracing::info!(
                    yellowstone_connected = now.yellowstone_connected,
                    events_received = now.events_received,
                    events_decoded = now.events_decoded,
                    decode_rejected = now.decode_rejected,
                    volume_updates = now.volume_updates,
                    swaps_pumpfun = now.swaps_pumpfun,
                    swaps_pumpswap = now.swaps_pumpswap,
                    stale_events = now.stale_events,
                    reserve_invalidations = now.reserve_invalidations,
                    reserve_revalidations = now.reserve_revalidations,
                    events_per_second = (delta_events as f64 / secs).round() as u64,
                    state_updates = now.state_updates,
                    state_updates_per_second = (delta_updates as f64 / secs).round() as u64,
                    slots_observed = now.slots_observed,
                    latest_slot = now.latest_slot,
                    active_market_states = now.active_market_states,
                    reconnects = now.reconnects,
                    invalid_events = now.invalid_events,
                    errors = now.errors,
                    worker_threads = now.worker_threads,
                    e2e_p50_us = now.latency_p50_ns as f64 / 1_000.0,
                    e2e_p99_us = now.latency_p99_ns as f64 / 1_000.0,
                    proc_p50_us = now.processing_p50_ns as f64 / 1_000.0,
                    proc_p99_us = now.processing_p99_ns as f64 / 1_000.0,
                    decode_p50_us = now.decode_p50_ns as f64 / 1_000.0,
                    decode_p99_us = now.decode_p99_ns as f64 / 1_000.0,
                    shard_activity = ?now.shard_activity,
                    "runtime telemetry",
                );
                previous = now;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_and_percentiles() {
        let m = Metrics::new(2, vec![100, 1_000, 10_000]);
        m.record_state_update(0, 50, 50); // bucket 100
        m.record_state_update(1, 500, 500); // bucket 1000
        m.record_state_update(1, 5_000, 5_000); // bucket 10000
        m.record_state_update(1, 50_000, 50_000); // overflow
        let s = m.snapshot();
        assert_eq!(s.state_updates, 4);
        assert_eq!(s.shard_activity, vec![1, 3]);
        assert_eq!(s.latency_samples, 4);
        assert_eq!(s.processing_samples, 4);
        assert_eq!(s.latency_p50_ns, 1_000);
        assert_eq!(s.latency_max_ns, 10_000);
    }

    #[test]
    fn thread_observation_counts_distinct_buckets() {
        let m = Metrics::new(1, vec![1]);
        m.observe_worker_thread(0);
        m.observe_worker_thread(0);
        assert_eq!(m.observed_worker_threads(), 1);
        m.observe_worker_thread(1);
        assert_eq!(m.observed_worker_threads(), 2);
    }
}
