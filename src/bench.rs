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

/// Pure protocol-decode throughput over real fixture bytes.
#[derive(Debug, Clone)]
pub struct DecodeBenchReport {
    pub decodes: u64,
    pub elapsed: Duration,
    pub decodes_per_second: f64,
    pub ns_per_decode: f64,
}

fn fixture_hex(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

/// Decode the real fixture payloads repeatedly to measure decode cost.
pub fn decode_microbench(iterations: u64) -> DecodeBenchReport {
    use crate::decode;
    let curve = fixture_hex(include_str!("../fixtures/pumpfun_bonding_curve.hex"));
    let pf_trade = fixture_hex(include_str!("../fixtures/pumpfun_trade_event.hex"));
    let pool = fixture_hex(include_str!("../fixtures/pumpswap_pool.hex"));
    let ps_buy = fixture_hex(include_str!("../fixtures/pumpswap_buy_event.hex"));
    let ps_sell = fixture_hex(include_str!("../fixtures/pumpswap_sell_event.hex"));
    let pf_program = crate::events::MarketKey(decode::pumpfun::PROGRAM_ID);
    let ps_program = crate::events::MarketKey(decode::pump_amm::PROGRAM_ID);

    let mut sink = 0u64;
    let start = Instant::now();
    for _ in 0..iterations {
        sink = sink.wrapping_add(
            decode::decode_account(&pf_program, &curve)
                .map_or(0, |d| d.base_reserve.unwrap_or(0) as u64),
        );
        sink = sink.wrapping_add(
            decode::decode_account(&ps_program, &pool)
                .map_or(0, |d| u64::from(d.base_mint.is_some())),
        );
        sink = sink.wrapping_add(decode::decode_event(&pf_trade).map_or(0, |s| s.quote_amount));
        sink = sink.wrapping_add(decode::decode_event(&ps_buy).map_or(0, |s| s.quote_amount));
        sink = sink.wrapping_add(decode::decode_event(&ps_sell).map_or(0, |s| s.quote_amount));
    }
    let elapsed = start.elapsed();
    let decodes = iterations.saturating_mul(5);
    let secs = elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
    std::hint::black_box(sink);
    DecodeBenchReport {
        decodes,
        elapsed,
        decodes_per_second: decodes as f64 / secs,
        ns_per_decode: elapsed.as_nanos() as f64 / decodes.max(1) as f64,
    }
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
        let period_ns = (1_000_000_000u64 / rate.max(1)).max(1);
        let mut t = tokio::time::interval(Duration::from_nanos(period_ns));
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
