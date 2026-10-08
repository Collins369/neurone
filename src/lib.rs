//! Neurone — Milestone 1 runtime foundation.
//!
//! This crate implements the live-data "nervous system" of Neurone:
//!
//! ```text
//! Solami Yellowstone gRPC
//!         -> event normalization
//!         -> deterministic shard routing
//!         -> parallel independent market states
//!         -> non-blocking telemetry
//! ```
//!
//! There is deliberately no trading logic here. Strategy, arming, capital
//! arbitration, Beam submission and position management belong to later
//! milestones. See `NEURONE_BLUEPRINT.md`.

pub mod bench;
pub mod clock;
pub mod config;
pub mod decode;
pub mod engine;
pub mod error;
pub mod events;
pub mod hash;
pub mod ingest;
pub mod market;
pub mod pyth;
pub mod quote;
pub mod reference;
pub mod runtime;
pub mod shard;
pub mod shutdown;
pub mod strategy;
pub mod telemetry;
pub mod validate;

pub use config::Config;
pub use engine::Engine;
pub use error::{Error, Result};
pub use events::{EventKind, MarketKey, NormalizedEvent};
pub use market::MarketState;
pub use runtime::run;
pub use shutdown::{shutdown_channel, Shutdown, ShutdownHandle};
pub use telemetry::{Metrics, MetricsSnapshot};
