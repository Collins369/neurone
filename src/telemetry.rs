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
    pub(crate) vault_balance_updates: AtomicU64,
    pub(crate) vault_balance_bootstraps: AtomicU64,
    pub(crate) strategy_evaluated: AtomicU64,
    pub(crate) strategy_qualified: AtomicU64,
    pub(crate) strategy_rejected_unsupported: AtomicU64,
    pub(crate) strategy_rejected_too_old: AtomicU64,
    pub(crate) strategy_rejected_insufficient_freshness: AtomicU64,
    pub(crate) strategy_rejected_already_pumped: AtomicU64,
    pub(crate) strategy_rejected_low_liquidity: AtomicU64,
    pub(crate) strategy_rejected_low_5m_volume: AtomicU64,
    pub(crate) strategy_rejected_mcap: AtomicU64,
    pub(crate) strategy_rejected_safety: AtomicU64,
    pub(crate) strategy_rejected_buy: AtomicU64,
    pub(crate) strategy_rejected_sell: AtomicU64,
    pub(crate) strategy_rejected_execution: AtomicU64,
    pub(crate) strategy_rejected_state: AtomicU64,
    pub(crate) strategy_rejected_consumed: AtomicU64,
    pub(crate) sol_usd_reference_updates: AtomicU64,
    pub(crate) markets_armed: AtomicU64,
    pub(crate) arm_refreshed: AtomicU64,
    pub(crate) arm_already_armed: AtomicU64,
    pub(crate) arm_invalidated: AtomicU64,
    pub(crate) arm_expired: AtomicU64,
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
    strategy_eval: Histogram,
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
    /// PumpSwap reserves established/updated from transaction
    /// `post_token_balances` (the late-market bootstrap path).
    pub vault_balance_updates: u64,
    /// Subset of `vault_balance_updates` that took a market from `Unknown` to
    /// `Known` (a true late-market bootstrap).
    pub vault_balance_bootstraps: u64,
    /// M4 strategy evaluations performed.
    pub markets_evaluated: u64,
    /// M4 markets that newly qualified.
    pub markets_qualified: u64,
    pub rejected_unsupported: u64,
    pub rejected_too_old: u64,
    /// Rejections where chain-proven creation/history could not be established.
    pub rejected_insufficient_freshness: u64,
    pub rejected_already_pumped: u64,
    pub rejected_low_liquidity: u64,
    pub rejected_low_5m_volume: u64,
    pub rejected_mcap: u64,
    pub rejected_safety: u64,
    pub rejected_buy: u64,
    pub rejected_sell: u64,
    pub rejected_execution: u64,
    pub rejected_state: u64,
    pub rejected_consumed: u64,
    /// Accepted Pyth SOL/USD reference updates (Yellowstone account stream).
    pub sol_usd_reference_updates: u64,
    /// M5: markets newly armed (Observing/Qualified -> Armed).
    pub markets_armed: u64,
    /// M5: armed contexts refreshed in place after a market-version change.
    pub arm_refreshed: u64,
    /// M5: arm requests already armed under the current version.
    pub arm_already_armed: u64,
    /// M5: armed contexts dropped because the market stopped qualifying.
    pub arm_invalidated: u64,
    /// M5: armed contexts dropped by the arm-expiry horizon.
    pub arm_expired: u64,
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
    /// M4 qualification evaluation latency.
    pub strategy_eval_p50_ns: u64,
    pub strategy_eval_p95_ns: u64,
    pub strategy_eval_p99_ns: u64,
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
            vault_balance_updates: AtomicU64::new(0),
            vault_balance_bootstraps: AtomicU64::new(0),
            strategy_evaluated: AtomicU64::new(0),
            strategy_qualified: AtomicU64::new(0),
            strategy_rejected_unsupported: AtomicU64::new(0),
            strategy_rejected_too_old: AtomicU64::new(0),
            strategy_rejected_insufficient_freshness: AtomicU64::new(0),
            strategy_rejected_already_pumped: AtomicU64::new(0),
            strategy_rejected_low_liquidity: AtomicU64::new(0),
            strategy_rejected_low_5m_volume: AtomicU64::new(0),
            strategy_rejected_mcap: AtomicU64::new(0),
            strategy_rejected_safety: AtomicU64::new(0),
            strategy_rejected_buy: AtomicU64::new(0),
            strategy_rejected_sell: AtomicU64::new(0),
            strategy_rejected_execution: AtomicU64::new(0),
            strategy_rejected_state: AtomicU64::new(0),
            strategy_rejected_consumed: AtomicU64::new(0),
            sol_usd_reference_updates: AtomicU64::new(0),
            markets_armed: AtomicU64::new(0),
            arm_refreshed: AtomicU64::new(0),
            arm_already_armed: AtomicU64::new(0),
            arm_invalidated: AtomicU64::new(0),
            arm_expired: AtomicU64::new(0),
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
            decode: Histogram::new(latency_bounds.clone()),
            strategy_eval: Histogram::new(latency_bounds),
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

    /// Count a PumpSwap market whose reserves were established/updated from
    /// transaction `post_token_balances` (late-market bootstrap).
    pub fn incr_vault_balance_update(&self) {
        self.vault_balance_updates.fetch_add(1, Ordering::Relaxed);
    }

    /// Count a PumpSwap market that went from `Unknown` to `Known` via
    /// transaction `post_token_balances`.
    pub fn incr_vault_balance_bootstrap(&self) {
        self.vault_balance_bootstraps
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Record one M4 qualification-evaluation duration, nanoseconds.
    pub fn observe_strategy_eval(&self, ns: u64) {
        self.strategy_eval.record(ns);
    }

    /// Count one M4 market evaluation.
    pub fn incr_strategy_evaluated(&self) {
        self.strategy_evaluated.fetch_add(1, Ordering::Relaxed);
    }

    /// Count a market that newly qualified.
    pub fn incr_strategy_qualified(&self) {
        self.strategy_qualified.fetch_add(1, Ordering::Relaxed);
    }

    /// Count a deterministic M4 rejection, bucketed by reason.
    pub fn incr_strategy_rejected(&self, reason: crate::strategy::RejectReason) {
        use crate::strategy::RejectReason::*;
        let counter = match reason {
            UnsupportedMarket => &self.strategy_rejected_unsupported,
            TokenTooOld => &self.strategy_rejected_too_old,
            InsufficientFreshnessData => &self.strategy_rejected_insufficient_freshness,
            AlreadyPumped => &self.strategy_rejected_already_pumped,
            LowLiquidity => &self.strategy_rejected_low_liquidity,
            Low5mVolume => &self.strategy_rejected_low_5m_volume,
            McapTooLow | McapTooHigh => &self.strategy_rejected_mcap,
            UnsafeToken | UnsafeMarket => &self.strategy_rejected_safety,
            BuyUnavailable => &self.strategy_rejected_buy,
            SellUnavailable => &self.strategy_rejected_sell,
            BadExecutionEconomics => &self.strategy_rejected_execution,
            StateUnknown | StateStale | StateInvalidated => &self.strategy_rejected_state,
            ReferenceUnavailable => &self.strategy_rejected_state,
            AlreadyConsumed => &self.strategy_rejected_consumed,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Count an accepted SOL/USD reference update.
    pub fn incr_sol_usd_reference(&self) {
        self.sol_usd_reference_updates
            .fetch_add(1, Ordering::Relaxed);
    }

    /// M5: a market newly armed (Observing/Qualified -> Armed).
    pub fn incr_markets_armed(&self) {
        self.markets_armed.fetch_add(1, Ordering::Relaxed);
    }

    /// M5: an armed context refreshed in place after a market-version change.
    pub fn incr_arm_refreshed(&self) {
        self.arm_refreshed.fetch_add(1, Ordering::Relaxed);
    }

    /// M5: an arm request that was already armed at the current version.
    pub fn incr_arm_already_armed(&self) {
        self.arm_already_armed.fetch_add(1, Ordering::Relaxed);
    }

    /// M5: an armed context dropped because the market stopped qualifying.
    pub fn incr_arm_invalidated(&self) {
        self.arm_invalidated.fetch_add(1, Ordering::Relaxed);
    }

    /// M5: an armed context dropped by the arm-expiry horizon.
    pub fn incr_arm_expired(&self) {
        self.arm_expired.fetch_add(1, Ordering::Relaxed);
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
            vault_balance_updates: self.vault_balance_updates.load(Ordering::Relaxed),
            vault_balance_bootstraps: self.vault_balance_bootstraps.load(Ordering::Relaxed),
            markets_evaluated: self.strategy_evaluated.load(Ordering::Relaxed),
            markets_qualified: self.strategy_qualified.load(Ordering::Relaxed),
            rejected_unsupported: self.strategy_rejected_unsupported.load(Ordering::Relaxed),
            rejected_too_old: self.strategy_rejected_too_old.load(Ordering::Relaxed),
            rejected_insufficient_freshness: self
                .strategy_rejected_insufficient_freshness
                .load(Ordering::Relaxed),
            rejected_already_pumped: self
                .strategy_rejected_already_pumped
                .load(Ordering::Relaxed),
            rejected_low_liquidity: self.strategy_rejected_low_liquidity.load(Ordering::Relaxed),
            rejected_low_5m_volume: self.strategy_rejected_low_5m_volume.load(Ordering::Relaxed),
            rejected_mcap: self.strategy_rejected_mcap.load(Ordering::Relaxed),
            rejected_safety: self.strategy_rejected_safety.load(Ordering::Relaxed),
            rejected_buy: self.strategy_rejected_buy.load(Ordering::Relaxed),
            rejected_sell: self.strategy_rejected_sell.load(Ordering::Relaxed),
            rejected_execution: self.strategy_rejected_execution.load(Ordering::Relaxed),
            rejected_state: self.strategy_rejected_state.load(Ordering::Relaxed),
            rejected_consumed: self.strategy_rejected_consumed.load(Ordering::Relaxed),
            sol_usd_reference_updates: self.sol_usd_reference_updates.load(Ordering::Relaxed),
            markets_armed: self.markets_armed.load(Ordering::Relaxed),
            arm_refreshed: self.arm_refreshed.load(Ordering::Relaxed),
            arm_already_armed: self.arm_already_armed.load(Ordering::Relaxed),
            arm_invalidated: self.arm_invalidated.load(Ordering::Relaxed),
            arm_expired: self.arm_expired.load(Ordering::Relaxed),
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
            strategy_eval_p50_ns: self.strategy_eval.quantile(0.50),
            strategy_eval_p95_ns: self.strategy_eval.quantile(0.95),
            strategy_eval_p99_ns: self.strategy_eval.quantile(0.99),
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
                    vault_balance_updates = now.vault_balance_updates,
                    vault_balance_bootstraps = now.vault_balance_bootstraps,
                    markets_evaluated = now.markets_evaluated,
                    markets_qualified = now.markets_qualified,
                    rejected_too_old = now.rejected_too_old,
                    rejected_insufficient_freshness = now.rejected_insufficient_freshness,
                    rejected_already_pumped = now.rejected_already_pumped,
                    rejected_low_liquidity = now.rejected_low_liquidity,
                    rejected_mcap = now.rejected_mcap,
                    rejected_low_5m_volume = now.rejected_low_5m_volume,
                    rejected_state = now.rejected_state,
                    rejected_buy = now.rejected_buy,
                    rejected_sell = now.rejected_sell,
                    rejected_reference = now.rejected_state,
                    sol_usd_reference_updates = now.sol_usd_reference_updates,
                    markets_armed = now.markets_armed,
                    arm_refreshed = now.arm_refreshed,
                    arm_invalidated = now.arm_invalidated,
                    arm_expired = now.arm_expired,
                    strategy_eval_p50_us = now.strategy_eval_p50_ns as f64 / 1_000.0,
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
