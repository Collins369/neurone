//! Runtime configuration.
//!
//! Precedence (highest first):
//!   1. Environment variables (`NEURONE_*`, plus `SOLAMI_GRPC_TOKEN`).
//!   2. TOML file (path from `NEURONE_CONFIG`, default `config/default.toml`).
//!   3. Compiled-in defaults.
//!
//! Secrets are never read from the TOML file: the Solami gRPC token is loaded
//! only from the environment so it cannot be committed accidentally. See
//! `.env.example` and `docs/SOLAMI_RESEARCH.md`.

use std::path::Path;

use serde::Deserialize;

use crate::error::{Error, Result};

/// Minimum latency buckets (nanoseconds) used by the telemetry histogram.
const DEFAULT_LATENCY_BOUNDARIES_NS: &[u64] = &[
    100,
    500,
    1_000,
    5_000,
    10_000,
    25_000,
    50_000,
    100_000,
    250_000,
    500_000,
    1_000_000,
    5_000_000,
    10_000_000,
    50_000_000,
    100_000_000,
    1_000_000_000,
];

/// Fully resolved runtime configuration.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub runtime: RuntimeConfig,
    pub ingest: IngestConfig,
    pub telemetry: TelemetryConfig,
    pub market: MarketConfig,
}

/// Market-state infrastructure settings (not strategy rules).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MarketConfig {
    /// A market's reserves are considered stale when the current slot exceeds
    /// `last_reserve_slot` by more than this many slots. This is an
    /// infrastructure freshness bound, not a trading threshold.
    pub reserve_stale_slots: u64,
}

/// Task structure and channel sizing.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RuntimeConfig {
    /// Number of independent market-state shards.
    pub shards: usize,
    /// Bounded capacity of each shard channel (backpressure boundary).
    pub shard_channel_capacity: usize,
    /// Tokio worker threads. Defaults to `max(2, available_parallelism)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_threads: Option<usize>,
}

/// Where events come from and how the stream is configured.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IngestConfig {
    /// `"solami"` (live Yellowstone gRPC) or `"simulated"` (offline generator).
    pub source: IngestSource,
    /// Yellowstone gRPC endpoint. Solami: `https://grpc.solami.dev:443`.
    pub endpoint: String,
    /// Commitment level requested in the `SubscribeRequest`.
    pub commitment: Commitment,
    /// `SubscribeRequestPing` cadence.
    pub ping_interval_ms: u64,
    /// Connection timeout for the initial TLS/gRPC handshake.
    pub connect_timeout_ms: u64,
    /// Initial reconnect backoff.
    pub reconnect_initial_ms: u64,
    /// Maximum reconnect backoff.
    pub reconnect_max_ms: u64,
    /// Reconnect if no update arrives for this long (stream health monitor).
    pub stale_stream_timeout_ms: u64,
    /// Resume with `from_slot = last_seen + 1` after a gap (Solami replays up
    /// to 3,500 slots back).
    pub replay_from_last_slot: bool,
    /// Subscription filters.
    pub filters: FilterConfig,
    /// Settings for the offline `simulated` source.
    pub simulated: SimulatedConfig,
}

/// Subscription filter configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FilterConfig {
    /// Subscribe to slot updates (interslot transitions).
    pub slot_updates: bool,
    /// Subscribe to block metadata (block height/time boundaries).
    pub block_meta: bool,
    /// Program IDs whose transactions should be streamed (bounded filter).
    pub transaction_programs: Vec<String>,
    /// Program IDs whose *owned accounts* should be streamed as markets.
    pub account_programs: Vec<String>,
    /// Explicit account pubkeys to track as markets.
    pub account_addresses: Vec<String>,
}

/// Offline generator settings (no live credential required).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SimulatedConfig {
    /// Number of synthetic markets to maintain.
    pub markets: usize,
    /// Events emitted per second.
    pub events_per_second: u64,
    /// Total events to emit before stopping (0 = run until shutdown).
    pub total_events: u64,
}

/// Telemetry reporter settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TelemetryConfig {
    /// How often the reporter logs a structured snapshot.
    pub report_interval_ms: u64,
    /// Upper latency bucket boundaries, nanoseconds.
    pub latency_boundaries_ns: Vec<u64>,
}

/// Ingestion source selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IngestSource {
    Solami,
    Simulated,
}

/// Yellowstone commitment level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Commitment {
    Processed,
    Confirmed,
    Finalized,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            shards: 8,
            shard_channel_capacity: 8_192,
            worker_threads: None,
        }
    }
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            source: IngestSource::Solami,
            // Solami Yellowstone gRPC endpoint (docs: /docs/grpc).
            endpoint: "https://grpc.solami.dev:443".to_string(),
            commitment: Commitment::Processed,
            ping_interval_ms: 15_000,
            connect_timeout_ms: 10_000,
            reconnect_initial_ms: 500,
            reconnect_max_ms: 30_000,
            stale_stream_timeout_ms: 30_000,
            replay_from_last_slot: true,
            filters: FilterConfig::default(),
            simulated: SimulatedConfig::default(),
        }
    }
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            slot_updates: true,
            block_meta: true,
            // Both venues are decoded in M2: the pump.fun bonding curve and the
            // pump.swap AMM. Program ids come from the official pump.fun IDLs.
            transaction_programs: vec![
                "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P".to_string(),
                "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA".to_string(),
            ],
            // Owner-scoped account subscriptions trigger a large startup
            // snapshot on Yellowstone, so they are opt-in. Markets are created
            // from decoded swap/create events by default; pin explicit account
            // addresses instead when you want account-driven state.
            account_programs: Vec::new(),
            account_addresses: Vec::new(),
        }
    }
}

impl Default for SimulatedConfig {
    fn default() -> Self {
        Self {
            markets: 512,
            events_per_second: 5_000,
            total_events: 0,
        }
    }
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            report_interval_ms: 5_000,
            latency_boundaries_ns: DEFAULT_LATENCY_BOUNDARIES_NS.to_vec(),
        }
    }
}

impl Default for MarketConfig {
    fn default() -> Self {
        // ~60s at 400ms slots.
        Self {
            reserve_stale_slots: 150,
        }
    }
}

impl Commitment {
    /// Map to the Yellowstone protobuf commitment enum value.
    pub fn to_proto(self) -> i32 {
        use yellowstone_grpc_proto::prelude::CommitmentLevel;
        match self {
            Commitment::Processed => CommitmentLevel::Processed as i32,
            Commitment::Confirmed => CommitmentLevel::Confirmed as i32,
            Commitment::Finalized => CommitmentLevel::Finalized as i32,
        }
    }
}

impl Config {
    /// Load configuration from `.env`, the TOML file, then environment overrides.
    pub fn load() -> Result<Self> {
        // Best-effort: a missing `.env` is normal in production.
        let _ = dotenvy::dotenv();

        let path = std::env::var("NEURONE_CONFIG").unwrap_or_else(|_| "config/default.toml".into());
        let mut config = if Path::new(&path).exists() {
            let raw = std::fs::read_to_string(&path)?;
            toml::from_str(&raw)
                .map_err(|e| Error::Config(format!("failed to parse {path}: {e}")))?
        } else {
            Config::default()
        };

        config.apply_env_overrides()?;
        config.validate()?;
        Ok(config)
    }

    fn apply_env_overrides(&mut self) -> Result<()> {
        if let Ok(v) = std::env::var("NEURONE_SOURCE") {
            self.ingest.source = parse_source(&v)?;
        }
        if let Ok(v) = std::env::var("SOLAMI_GRPC_ENDPOINT")
            .or_else(|_| std::env::var("SOLAMI_YELLOWSTONE_ENDPOINT"))
        {
            if !v.trim().is_empty() {
                self.ingest.endpoint = v;
            }
        }
        if let Ok(v) = std::env::var("NEURONE_SHARDS") {
            self.runtime.shards = v
                .parse()
                .map_err(|e| Error::Config(format!("NEURONE_SHARDS: {e}")))?;
        }
        if let Ok(v) = std::env::var("NEURONE_WORKER_THREADS") {
            self.runtime.worker_threads = Some(
                v.parse()
                    .map_err(|e| Error::Config(format!("NEURONE_WORKER_THREADS: {e}")))?,
            );
        }
        Ok(())
    }

    /// Reject configurations that would silently misbehave at runtime.
    pub fn validate(&self) -> Result<()> {
        if self.runtime.shards == 0 {
            return Err(Error::Config("runtime.shards must be >= 1".into()));
        }
        if self.runtime.shard_channel_capacity == 0 {
            return Err(Error::Config(
                "runtime.shard_channel_capacity must be >= 1".into(),
            ));
        }
        if self.ingest.ping_interval_ms == 0 {
            return Err(Error::Config("ingest.ping_interval_ms must be > 0".into()));
        }
        if self.ingest.stale_stream_timeout_ms <= self.ingest.ping_interval_ms {
            return Err(Error::Config(
                "ingest.stale_stream_timeout_ms must exceed ping_interval_ms".into(),
            ));
        }
        if self.telemetry.report_interval_ms == 0 {
            return Err(Error::Config(
                "telemetry.report_interval_ms must be > 0".into(),
            ));
        }
        if self.telemetry.latency_boundaries_ns.is_empty() {
            return Err(Error::Config(
                "telemetry.latency_boundaries_ns must not be empty".into(),
            ));
        }
        let mut prev = 0u64;
        for &b in &self.telemetry.latency_boundaries_ns {
            if b <= prev {
                return Err(Error::Config(
                    "telemetry.latency_boundaries_ns must be strictly increasing".into(),
                ));
            }
            prev = b;
        }
        if self.ingest.source == IngestSource::Solami {
            // Validate program/account pubkeys eagerly so the failure is
            // deterministic and happens before we open a socket.
            for (label, list) in [
                (
                    "transaction_program",
                    &self.ingest.filters.transaction_programs,
                ),
                ("account_program", &self.ingest.filters.account_programs),
                ("account_address", &self.ingest.filters.account_addresses),
            ] {
                for p in list {
                    let decoded = bs58::decode(p)
                        .into_vec()
                        .map_err(|e| Error::Config(format!("invalid {label} `{p}`: {e}")))?;
                    if decoded.len() != crate::events::PUBKEY_LEN {
                        return Err(Error::Config(format!(
                            "invalid {label} `{p}`: expected 32 bytes, got {}",
                            decoded.len()
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// Read the Solami gRPC token from the environment.
    ///
    /// Returns `None` when running the simulated source. Never reads from the
    /// TOML file so the secret cannot be committed.
    pub fn solami_token(&self) -> Result<Option<String>> {
        if self.ingest.source != IngestSource::Solami {
            return Ok(None);
        }
        let token = std::env::var("SOLAMI_GRPC_TOKEN")
            .or_else(|_| std::env::var("SOLAMI_API_KEY"))
            .ok()
            .filter(|t| !t.trim().is_empty());
        match token {
            Some(t) => Ok(Some(t)),
            None => Err(Error::Config(
                "source=solami requires SOLAMI_GRPC_TOKEN (or SOLAMI_API_KEY) in the environment; \
                 see .env.example"
                    .into(),
            )),
        }
    }

    /// Effective tokio worker-thread count.
    pub fn effective_worker_threads(&self) -> usize {
        self.runtime.worker_threads.unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
                .max(2)
        })
    }
}

fn parse_source(v: &str) -> Result<IngestSource> {
    match v.to_ascii_lowercase().as_str() {
        "solami" | "live" => Ok(IngestSource::Solami),
        "simulated" | "sim" | "mock" => Ok(IngestSource::Simulated),
        other => Err(Error::Config(format!(
            "unknown source `{other}` (expected `solami` or `simulated`)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        Config::default().validate().expect("default config valid");
    }

    #[test]
    fn zero_shards_rejected() {
        let mut c = Config::default();
        c.runtime.shards = 0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn unsorted_latency_buckets_rejected() {
        let mut c = Config::default();
        c.telemetry.latency_boundaries_ns = vec![10, 5];
        assert!(c.validate().is_err());
    }

    #[test]
    fn commitment_maps_to_proto() {
        assert_eq!(
            Commitment::Processed.to_proto(),
            yellowstone_grpc_proto::prelude::CommitmentLevel::Processed as i32
        );
    }
}
