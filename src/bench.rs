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

/// Quote-engine latency over a warm market state.
#[derive(Debug, Clone)]
pub struct QuoteBenchReport {
    pub quotes: u64,
    pub p50_ns: u64,
    pub p95_ns: u64,
    pub p99_ns: u64,
    pub ns_per_quote: f64,
}

/// M4 strategy evaluation + rolling-volume update latency.
#[derive(Debug, Clone)]
pub struct StrategyBenchReport {
    pub evaluations: u64,
    pub eval_p50_ns: u64,
    pub eval_p95_ns: u64,
    pub eval_p99_ns: u64,
    pub ns_per_eval: f64,
    pub volume_updates: u64,
    pub ns_per_volume_update: f64,
}

fn percentiles(samples: &mut [u64]) -> (u64, u64, u64) {
    if samples.is_empty() {
        return (0, 0, 0);
    }
    samples.sort_unstable();
    let at = |q: f64| samples[((samples.len() - 1) as f64 * q) as usize];
    (at(0.50), at(0.95), at(0.99))
}

/// Measure M4 qualification evaluation and rolling-volume update latency.
pub fn strategy_microbench(iterations: u64) -> StrategyBenchReport {
    use crate::decode::Venue;
    use crate::events::MarketKey;
    use crate::strategy::{self, Decision, StrategyConfig};

    let cfg = StrategyConfig {
        sol_usd_price_micros: Some(1_000_000_000),
        ..Default::default()
    };
    let mut m = crate::market::MarketState::new(MarketKey([7u8; 32]), 0);
    m.venue = Some(Venue::PumpFun);
    m.base_mint = Some(MarketKey([0xA1u8; 32]));
    m.token_total_supply = 5_000_000_000_000_000;
    m.virtual_base_reserve = 1_000_000_000_000_000;
    m.virtual_quote_reserve = 1_000_000_000;
    m.base_reserve = 800_000_000_000_000;
    m.quote_reserve = 5_000_000_000;
    m.reserves_known = true;
    m.last_reserve_slot = 1_000;
    m.last_slot = 1_000;
    m.last_fee_bps = Some(95);
    m.created_at_slot = 900;
    m.volume.record(990, true, 1_000_000_000, 1);

    let n = iterations.max(1);
    let mut eval_samples = Vec::with_capacity(n as usize);
    let mut sink = 0u64;
    for _ in 0..n {
        let t = Instant::now();
        let d = strategy::evaluate(&m, 1_000, 150, &cfg, cfg.sol_usd_price_micros);
        eval_samples.push(t.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64);
        sink = sink.wrapping_add(matches!(d, Decision::Qualified) as u64);
    }
    let eval_total: u64 = eval_samples.iter().sum();
    let eval = StrategyBenchReport {
        evaluations: n,
        eval_p50_ns: percentiles(&mut eval_samples).0,
        eval_p95_ns: percentiles(&mut eval_samples).1,
        eval_p99_ns: percentiles(&mut eval_samples).2,
        ns_per_eval: eval_total as f64 / n as f64,
        volume_updates: n,
        ns_per_volume_update: 0.0,
    };
    // Rolling-volume update latency.
    let t = Instant::now();
    for i in 0..n {
        m.volume.record(1_000 + i, true, 1, 1);
    }
    let vns = t.elapsed().as_nanos() as f64 / n as f64;
    std::hint::black_box(sink);
    std::hint::black_box(eval_total);
    StrategyBenchReport {
        ns_per_volume_update: vns,
        ..eval
    }
}

/// Measure `quote_buy`/`quote_sell` latency on a representative market.
pub fn quote_microbench(iterations: u64) -> QuoteBenchReport {
    use crate::decode::Venue;
    use crate::quote::{quote, Side};
    let mut m = crate::market::MarketState::new(crate::events::MarketKey([9u8; 32]), 0);
    m.venue = Some(Venue::PumpSwap);
    // A balanced pool so both sides produce a non-zero output at every size.
    m.base_reserve = 1_000_000_000_000;
    m.quote_reserve = 1_000_000_000;
    m.reserves_known = true;
    m.last_reserve_slot = 1_000;

    let mut samples = Vec::with_capacity(iterations as usize);
    let mut sink = 0u128;
    for i in 0..iterations {
        let side = if i % 2 == 0 { Side::Buy } else { Side::Sell };
        let input = 1_000 + (i % 1_000_000);
        let t = Instant::now();
        match quote(&m, side, input, 25, 1_000, 150) {
            Ok(q) => {
                samples.push(t.elapsed().as_nanos() as u64);
                sink = sink.wrapping_add(q.net_output);
            }
            // Degenerate sizes are counted as invalid, not fatal.
            Err(_) => {
                samples.push(t.elapsed().as_nanos() as u64);
            }
        }
    }
    std::hint::black_box(sink);
    samples.sort_unstable();
    let pick = |q: f64| -> u64 {
        if samples.is_empty() {
            return 0;
        }
        let idx = ((samples.len() as f64 * q).ceil() as usize).saturating_sub(1);
        samples[idx.min(samples.len() - 1)]
    };
    let total: u64 = samples.iter().sum();
    QuoteBenchReport {
        quotes: iterations,
        p50_ns: pick(0.50),
        p95_ns: pick(0.95),
        p99_ns: pick(0.99),
        ns_per_quote: total as f64 / iterations.max(1) as f64,
    }
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
    let (engine, shard_handles) = Engine::start_with_strategy(
        config.runtime.shards,
        config.runtime.shard_channel_capacity,
        Arc::clone(&metrics),
        shutdown,
        Arc::new(config.strategy.clone()),
        config.market.reserve_stale_slots,
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
