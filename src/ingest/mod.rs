//! Event ingestion.
//!
//! Two interchangeable sources produce the same normalized event types:
//! * [`solami`] — the live Solami Yellowstone gRPC stream (production path).
//! * [`simulated`] — a deterministic offline generator used for tests and for
//!   exercising the parallel engine without a live credential.
//!
//! The ingestion boundary is the only place that understands Yellowstone
//! protobuf; everything downstream sees [`crate::events::NormalizedEvent`].

pub mod simulated;
pub mod solami;

use std::sync::Arc;

use crate::config::{Config, IngestSource};
use crate::engine::Engine;
use crate::error::Result;
use crate::shutdown::Shutdown;
use crate::telemetry::Metrics;

/// Run the configured ingestion source until shutdown.
pub async fn run(
    config: &Config,
    engine: Engine,
    metrics: Arc<Metrics>,
    shutdown: Shutdown,
) -> Result<()> {
    match config.ingest.source {
        IngestSource::Solami => {
            let token = solami::load_token()?;
            let connector = solami::TonicConnector::new(&config.ingest, token);
            solami::run(&config.ingest, &connector, engine, metrics, shutdown).await
        }
        IngestSource::Simulated => simulated::run(&config.ingest, engine, metrics, shutdown).await,
    }
}
