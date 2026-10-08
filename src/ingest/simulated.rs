//! Deterministic offline event generator.
//!
//! It produces the same normalized event types the live path produces, so the
//! sharded engine, telemetry and shutdown behaviour can be exercised end to
//! end without a live Solami credential. The generator is a pure function of a
//! seed and step counter, which is what makes state-determinism tests possible.

use std::sync::Arc;
use std::time::Duration;

use crate::config::IngestConfig;
use crate::engine::Engine;
use crate::error::Result;
use crate::events::{
    AccountUpdate, BlockMetaUpdate, EventKind, MarketKey, NormalizedEvent, SlotStatusKind,
    SlotUpdate, TransactionUpdate,
};
use crate::shutdown::Shutdown;
use crate::telemetry::Metrics;

/// Seed used when none is supplied, for reproducible runs.
pub const DEFAULT_SEED: u64 = 0x004E_4555_524F_4E45; // "NEURONE"

/// Deterministic synthetic event source.
pub struct SimulatedSource {
    markets: Vec<MarketKey>,
    write_versions: Vec<u64>,
    rng: u64,
    step: u64,
    slot: u64,
}

impl SimulatedSource {
    /// Create a source maintaining `markets` synthetic market identities.
    pub fn new(markets: usize, seed: u64) -> Self {
        let markets: Vec<MarketKey> = (0..markets.max(1)).map(synthetic_key).collect();
        let write_versions = vec![0u64; markets.len()];
        Self {
            markets,
            write_versions,
            rng: seed | 1,
            step: 0,
            slot: 200_000_000,
        }
    }

    /// Produce the next event in the deterministic sequence.
    pub fn next_event(&mut self) -> NormalizedEvent {
        self.step += 1;
        // Slot tick roughly every 200 events.
        if self.step.is_multiple_of(200) {
            self.slot += 1;
            return NormalizedEvent::new(EventKind::Slot(SlotUpdate {
                slot: self.slot,
                parent: Some(self.slot - 1),
                status: SlotStatusKind::Processed,
            }));
        }
        if self.step.is_multiple_of(500) {
            return NormalizedEvent::new(EventKind::BlockMeta(BlockMetaUpdate {
                slot: self.slot,
                block_height: Some(self.slot - 100),
                block_time: Some(1_700_000_000 + self.slot as i64),
                executed_transaction_count: 1_000,
            }));
        }

        let idx = (self.next_rand() as usize) % self.markets.len();
        let key = self.markets[idx];
        if self.next_rand() % 10 < 8 {
            // Account update: bump the per-market write version and digest.
            self.write_versions[idx] += 1;
            let wv = self.write_versions[idx];
            NormalizedEvent::new(EventKind::Account(AccountUpdate {
                pubkey: key,
                slot: self.slot,
                owner: Some(synthetic_key(usize::MAX)),
                lamports: 1_000 + wv,
                data_len: 64,
                data_digest: wv.wrapping_mul(0x9E37_79B9_7F4A_7C15),
                write_version: wv,
                is_startup: false,
                txn_signature: None,
                decoded: None,
            }))
        } else {
            // Transaction touching this market (plus a couple of neighbours).
            let sig = synthetic_signature(self.step);
            let mut keys = vec![key, self.markets[(idx + 1) % self.markets.len()]];
            let is_buy = self.next_rand().is_multiple_of(2);
            let quote = 1_000 + (self.step % 50_000);
            let base = 1_000_000 + (self.step % 1_000_000);
            let swap = crate::decode::DecodedSwap {
                venue: crate::decode::Venue::PumpFun,
                market_key: key,
                base_mint: Some(key),
                quote_mint: None,
                is_buy,
                base_amount: base,
                quote_amount: quote,
                user_quote_amount: quote,
                base_reserve: Some(800_000_000 + self.step),
                quote_reserve: Some(5_000_000_000 + self.step),
                virtual_base_reserve: Some(1_073_000_000_000_000),
                virtual_quote_reserve: Some(30_000_000_000),
                fee_quote: quote / 100,
                fee_bps: Some(95),
                timestamp: None,
                ix_name: None,
            };
            keys.sort_unstable();
            keys.dedup();
            NormalizedEvent::new(EventKind::Transaction(TransactionUpdate {
                signature: sig,
                slot: self.slot,
                index: self.step,
                is_vote: false,
                success: true,
                keys,
                swaps: vec![swap],
                creates: Vec::new(),
                decode_rejected: 0,
                vault_balances: Vec::new(),
                has_reserve_mutation: false,
                has_sweep: false,
            }))
        }
    }

    fn next_rand(&mut self) -> u64 {
        // SplitMix64: tiny, deterministic, good enough for selection.
        self.rng = self.rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Run the simulated source at the configured rate until shutdown/completion.
pub async fn run(
    cfg: &IngestConfig,
    engine: Engine,
    metrics: Arc<Metrics>,
    shutdown: Shutdown,
) -> Result<()> {
    let sim = &cfg.simulated;
    tracing::info!(
        markets = sim.markets,
        events_per_second = sim.events_per_second,
        total_events = sim.total_events,
        "starting simulated ingestion"
    );
    metrics.set_connected(true);

    let mut source = SimulatedSource::new(sim.markets, DEFAULT_SEED);
    let eps = sim.events_per_second.max(1);
    // Clamp to >= 1ns so an absurd rate cannot produce a zero-duration interval.
    let period_ns = (1_000_000_000u64 / eps).max(1);
    let mut ticker = tokio::time::interval(Duration::from_nanos(period_ns));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);

    let mut emitted: u64 = 0;
    loop {
        if shutdown.is_cancelled() {
            break;
        }
        if sim.total_events != 0 && emitted >= sim.total_events {
            tracing::info!(emitted, "simulated source finished");
            break;
        }
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = ticker.tick() => {
                let event = source.next_event();
                emitted += 1;
                metrics.incr_received();
                metrics.incr_normalized();
                engine.route(event).await?;
            }
        }
    }
    metrics.set_connected(false);
    Ok(())
}

fn synthetic_key(index: usize) -> MarketKey {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&(index as u64).to_le_bytes());
    bytes[8] = 0xA5;
    bytes[31] = 0x5A;
    MarketKey(bytes)
}

fn synthetic_signature(step: u64) -> [u8; 64] {
    let mut sig = [0u8; 64];
    for (i, chunk) in sig.chunks_mut(8).enumerate() {
        chunk.copy_from_slice(&(step.wrapping_mul(i as u64 + 1)).to_le_bytes());
    }
    sig
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_deterministic() {
        let mut a = SimulatedSource::new(8, 1);
        let mut b = SimulatedSource::new(8, 1);
        for _ in 0..1_000 {
            let ea = a.next_event();
            let eb = b.next_event();
            assert_eq!(format!("{:?}", ea.kind), format!("{:?}", eb.kind));
        }
    }
}
