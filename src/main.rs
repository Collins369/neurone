//! Neurone entry point (Milestone 1).
//!
//! Usage:
//!   neurone run     # live/simulated runtime (source from config/env)
//!   neurone bench   # offline throughput/latency harness

use std::process::ExitCode;

use anyhow::Context;
use neurone::config::Config;

fn main() -> ExitCode {
    init_tracing();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!("fatal: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let config = Config::load().context("loading configuration")?;
    config.validate().context("validating configuration")?;

    let command = std::env::args().nth(1).unwrap_or_else(|| "run".to_string());
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(config.effective_worker_threads())
        .enable_all()
        .thread_name("neurone-worker")
        .build()
        .context("building tokio runtime")?;

    match command.as_str() {
        "run" => runtime
            .block_on(neurone::run(config))
            .map_err(anyhow::Error::from),
        "bench" => runtime.block_on(async {
            let events: u64 = std::env::var("NEURONE_BENCH_EVENTS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(200_000);
            let rate: Option<u64> = std::env::var("NEURONE_BENCH_RATE")
                .ok()
                .and_then(|v| v.parse().ok());
            let report = neurone::bench::run(&config, events, rate).await?;
            println!(
                "bench: events={} elapsed={:.3}s events_per_second={:.0} state_updates={} worker_threads={} \
                 e2e(p50={}us p95={}us p99={}us max={}us) proc(p50={}us p99={}us max={}us) shards={:?}",
                report.events,
                report.elapsed.as_secs_f64(),
                report.events_per_second,
                report.metrics.state_updates,
                report.worker_threads,
                report.metrics.latency_p50_ns / 1_000,
                report.metrics.latency_p95_ns / 1_000,
                report.metrics.latency_p99_ns / 1_000,
                report.metrics.latency_max_ns / 1_000,
                report.metrics.processing_p50_ns / 1_000,
                report.metrics.processing_p99_ns / 1_000,
                report.metrics.processing_max_ns / 1_000,
                report.metrics.shard_activity,
            );
            let (_bounds, counts) = report.latency_buckets;
            println!("e2e latency histogram counts={counts:?}");
            Ok::<(), anyhow::Error>(())
        }),
        "check" => {
            // Offline check: validate configuration and subscription shape
            // without opening a socket.
            let req = neurone::ingest::solami::build_subscribe_request(
                &config.ingest,
                Some(1_000),
            )?;
            println!(
                "config ok: source={:?} endpoint={} filters(accounts={} txs={} slots={} blocks_meta={}) commitment={:?} from_slot={:?}",
                config.ingest.source,
                config.ingest.endpoint,
                req.accounts.len(),
                req.transactions.len(),
                req.slots.len(),
                req.blocks_meta.len(),
                config.ingest.commitment,
                req.from_slot,
            );
            Ok(())
        }
        other => anyhow::bail!("unknown command `{other}` (expected run|bench|check)"),
    }
}

fn init_tracing() {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("neurone=info,info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(true))
        .init();
}
