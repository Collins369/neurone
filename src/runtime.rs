//! Runtime assembly and supervision.
//!
//! ```text
//! ingestion ──route──► shard 0
//!                    ► shard 1
//!                    ► shard N
//! telemetry reporter ──reads──► atomic metrics (never blocks the hot path)
//! ```
//!
//! The supervisor owns the shutdown signal: `Ctrl-C`, or the premature exit of
//! a critical task, triggers a cooperative shutdown of every component.

use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::engine::Engine;
use crate::error::{Error, Result};
use crate::ingest;
use crate::shutdown::shutdown_channel;
use crate::telemetry::{self, Metrics};

/// Run Neurone until shutdown, then drain every task cleanly.
pub async fn run(config: Config) -> Result<()> {
    config.validate()?;
    let metrics = Metrics::new(
        config.runtime.shards,
        config.telemetry.latency_boundaries_ns.clone(),
    );
    let (shutdown_handle, shutdown) = shutdown_channel();

    let (engine, shard_handles) = Engine::start_with_strategy(
        config.runtime.shards,
        config.runtime.shard_channel_capacity,
        Arc::clone(&metrics),
        shutdown.clone(),
        Arc::new(config.strategy.clone()),
        config.market.reserve_stale_slots,
    );

    tracing::info!(
        shards = config.runtime.shards,
        source = ?config.ingest.source,
        worker_threads = config.effective_worker_threads(),
        "Neurone runtime starting"
    );

    // The ingestion task owns its configuration copy so it can outlive the
    // borrow of `config` taken by the reporter setup below.
    let ingest_config = config.clone();
    let ingest_metrics = Arc::clone(&metrics);
    let ingest_shutdown = shutdown.clone();
    let mut ingest_handle = tokio::spawn(async move {
        ingest::run(&ingest_config, engine, ingest_metrics, ingest_shutdown).await
    });
    let report_handle = tokio::spawn(telemetry::report_loop(
        Arc::clone(&metrics),
        Duration::from_millis(config.telemetry.report_interval_ms),
        shutdown.clone(),
    ));

    // Wait for either an operator shutdown or an ingestion task exit.
    let ingest_result = tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            tracing::info!("Ctrl-C received; shutting down");
            None
        }
        joined = &mut ingest_handle => Some(joined),
    };

    // Whatever ended the wait, request shutdown and let tasks drain.
    shutdown_handle.trigger();

    // Await ingestion. If Ctrl-C won the select, the handle is still live and
    // will complete once it observes the shutdown signal.
    let ingest_error = match ingest_result {
        Some(joined) => join_ingest(joined)?,
        None => join_ingest(ingest_handle.await)?,
    };

    for (i, handle) in shard_handles.into_iter().enumerate() {
        handle
            .await
            .map_err(|e| Error::Ingest(format!("shard {i} panicked: {e}")))?;
    }
    report_handle
        .await
        .map_err(|e| Error::Ingest(format!("telemetry reporter panicked: {e}")))?;

    let final_snapshot = metrics.snapshot();
    tracing::info!(?final_snapshot, "Neurone shut down cleanly");

    if let Some(err) = ingest_error {
        return Err(err);
    }
    Ok(())
}

fn join_ingest(
    joined: std::result::Result<Result<()>, tokio::task::JoinError>,
) -> Result<Option<Error>> {
    match joined {
        Ok(Ok(())) => Ok(None),
        Ok(Err(e)) => Ok(Some(e)),
        Err(e) => Ok(Some(Error::Ingest(format!("ingestion task panicked: {e}")))),
    }
}
