//! Experimental SOL/USD reference derivation from Yellowstone-observed on-chain
//! SOL/USDC market state.
//!
//! **This module is NOT wired into M4.** The M4 SOL/USD investigation
//! (`docs/M4_AGE_DRAWDOWN_SOL_USD_REPORT.md`) found that the venues Neurone
//! currently subscribes to (pump.fun bonding curves and pump.swap `pump_amm`
//! pools) do **not** carry a SOL/USDC market, so no trustworthy Yellowstone-only
//! reference could be proven from bounded live data. M4 therefore keeps the
//! operator-supplied static `sol_usd_price_micros`.
//!
//! This is a bounded, pure, integer-only prototype kept for a future
//! authorized investigation: given a SOL/USDC pool's raw reserves it derives USD
//! per SOL and can track freshness. It performs no I/O and fails closed.

use crate::events::MarketKey;

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use crate::pyth::SolUsdPriceUpdate;

/// USDC has 6 decimals.
pub const QUOTE_DECIMALS: u32 = 6;
/// SOL has 9 decimals.
pub const BASE_DECIMALS: u32 = 9;

/// Derive SOL/USD (micros) from a SOL/USDC pool's raw reserves.
///
/// With SOL as base (9 decimals) and USDC as quote (6 decimals):
///
/// ```text
/// usd_per_sol = (usdc_raw / 10^6) / (sol_raw / 10^9) = usdc_raw * 10^3 / sol_raw
/// usd_micros  = usd_per_sol * 10^6                     = usdc_raw * 10^9 / sol_raw
/// ```
///
/// Integer-only. `None` when either reserve is zero or the arithmetic overflows.
pub fn sol_usd_micros_from_reserves(sol_raw: u128, usdc_raw: u128) -> Option<u64> {
    if sol_raw == 0 || usdc_raw == 0 {
        return None;
    }
    let scale = 10u128.checked_pow(BASE_DECIMALS + 6 - QUOTE_DECIMALS)?; // 10^9
    let micros = usdc_raw.checked_mul(scale)?.checked_div(sol_raw)?;
    u64::try_from(micros).ok()
}

/// A derived SOL/USD reference observation from one on-chain source.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SolUsdReference {
    price_micros: u64,
    source: MarketKey,
    slot: u64,
}

impl SolUsdReference {
    /// Build a reference from a source market's reserves at `slot`.
    pub fn from_reserves(
        source: MarketKey,
        sol_raw: u128,
        usdc_raw: u128,
        slot: u64,
    ) -> Option<Self> {
        let price_micros = sol_usd_micros_from_reserves(sol_raw, usdc_raw)?;
        Some(Self {
            price_micros,
            source,
            slot,
        })
    }

    pub fn price_micros(&self) -> u64 {
        self.price_micros
    }

    pub fn source(&self) -> MarketKey {
        self.source
    }

    pub fn slot(&self) -> u64 {
        self.slot
    }

    /// A reference is usable only within the freshness bound. Stale or
    /// future/zero state fails closed.
    pub fn is_fresh(&self, current_slot: u64, stale_slots: u64) -> bool {
        self.price_micros > 0
            && self.slot <= current_slot
            && current_slot.saturating_sub(self.slot) <= stale_slots
    }
}

/// Live SOL/USD reference state, updated by the Yellowstone ingestion task from
/// Pyth `PriceUpdateV2` account updates and read (lock-free, allocation-free) by
/// the strategy.
///
/// All fields are plain atomics: there is exactly one writer (ingestion) and
/// many readers (shards). `received_ns` is written last with `Release` and read
/// first with `Acquire`, so whenever a reader observes a fresh `received_ns` the
/// price fields are at least as new. `price_micros == 0` means "no valid
/// reference yet" (fail closed).
#[derive(Debug)]
pub struct SolUsdReferenceState {
    price_micros: AtomicU64,
    conf_micros: AtomicU64,
    publish_time: AtomicI64,
    posted_slot: AtomicU64,
    received_ns: AtomicU64,
    version: AtomicU64,
}

impl Default for SolUsdReferenceState {
    fn default() -> Self {
        Self::new()
    }
}

impl SolUsdReferenceState {
    pub fn new() -> Self {
        Self {
            price_micros: AtomicU64::new(0),
            conf_micros: AtomicU64::new(0),
            publish_time: AtomicI64::new(0),
            posted_slot: AtomicU64::new(0),
            received_ns: AtomicU64::new(0),
            version: AtomicU64::new(0),
        }
    }

    /// Apply a decoded Pyth update. Returns `true` if accepted.
    ///
    /// Rejects (fail closed, no state change): invalid/zero/negative price,
    /// non-`Full` verification, or a `publish_time` not newer than the last one
    /// (out-of-order or duplicate).
    pub fn update(&self, u: &SolUsdPriceUpdate, received_ns: u64) -> bool {
        let Some(price) = u.price_micros() else {
            return false;
        };
        let prev_pub = self.publish_time.load(Ordering::Relaxed);
        if u.publish_time <= prev_pub {
            return false; // duplicate / out-of-order
        }
        self.price_micros.store(price, Ordering::Relaxed);
        self.conf_micros
            .store(u.conf_micros().unwrap_or(0), Ordering::Relaxed);
        self.publish_time.store(u.publish_time, Ordering::Relaxed);
        self.posted_slot.store(u.posted_slot, Ordering::Relaxed);
        self.version.fetch_add(1, Ordering::Relaxed);
        // Publish the validity marker last.
        self.received_ns.store(received_ns, Ordering::Release);
        true
    }

    /// Monotonic ns at which the last accepted update was received (0 = none).
    pub fn received_ns(&self) -> u64 {
        self.received_ns.load(Ordering::Acquire)
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Relaxed)
    }

    /// Resolve the current SOL/USD price in micros, or `None` when the reference
    /// is absent, invalid, or older than `max_staleness_ms` (fail closed).
    pub fn read_micros(&self, now_ns: u64, max_staleness_ms: u64) -> Option<u64> {
        let received = self.received_ns.load(Ordering::Acquire);
        if received == 0 {
            return None;
        }
        let age_ns = now_ns.saturating_sub(received);
        if age_ns > max_staleness_ms.saturating_mul(1_000_000) {
            return None;
        }
        let price = self.price_micros.load(Ordering::Relaxed);
        (price > 0).then_some(price)
    }

    /// `(price_micros, conf_micros, publish_time, posted_slot, received_ns, version)`
    /// for diagnostics/telemetry.
    pub fn snapshot(&self) -> (u64, u64, i64, u64, u64, u64) {
        (
            self.price_micros.load(Ordering::Relaxed),
            self.conf_micros.load(Ordering::Relaxed),
            self.publish_time.load(Ordering::Relaxed),
            self.posted_slot.load(Ordering::Relaxed),
            self.received_ns.load(Ordering::Acquire),
            self.version.load(Ordering::Relaxed),
        )
    }
}

#[cfg(test)]
mod state_tests {
    use super::*;

    fn upd(price: i64, publish_time: i64) -> SolUsdPriceUpdate {
        SolUsdPriceUpdate {
            price,
            conf: 4_000_000,
            exponent: -8,
            publish_time,
            prev_publish_time: publish_time - 1,
            ema_price: price,
            ema_conf: 0,
            posted_slot: 100,
            full_verification: true,
        }
    }

    #[test]
    fn unknown_until_first_update() {
        let s = SolUsdReferenceState::new();
        assert_eq!(s.read_micros(1_000_000_000, 120_000), None);
        assert_eq!(s.version(), 0);
    }

    #[test]
    fn fresh_update_is_readable_then_goes_stale() {
        let s = SolUsdReferenceState::new();
        let t0 = 1_000_000_000_000u64; // ns
        assert!(s.update(&upd(11_125_337_300, 100), t0));
        assert_eq!(s.read_micros(t0, 120_000), Some(111_253_373));
        // Exactly at the bound: still fresh.
        assert_eq!(
            s.read_micros(t0 + 120_000 * 1_000_000, 120_000),
            Some(111_253_373)
        );
        // One ms past the bound: stale -> fail closed.
        assert_eq!(s.read_micros(t0 + 120_000 * 1_000_000 + 1, 120_000), None);
    }

    #[test]
    fn duplicate_and_out_of_order_rejected() {
        let s = SolUsdReferenceState::new();
        assert!(s.update(&upd(11_000_000_000, 100), 1));
        assert!(
            !s.update(&upd(11_000_000_000, 100), 2),
            "duplicate publish_time"
        );
        assert!(!s.update(&upd(11_500_000_000, 99), 3), "older publish_time");
        assert_eq!(s.version(), 1);
        assert!(s.update(&upd(11_500_000_000, 101), 4));
        assert_eq!(s.version(), 2);
        assert_eq!(s.read_micros(4, 120_000), Some(115_000_000));
    }

    #[test]
    fn invalid_update_does_not_change_state() {
        let s = SolUsdReferenceState::new();
        assert!(s.update(&upd(11_000_000_000, 100), 1));
        let before = s.snapshot();
        // Zero price -> invalid.
        assert!(!s.update(&upd(0, 101), 2));
        // Partial verification -> invalid.
        let mut p = upd(11_200_000_000, 102);
        p.full_verification = false;
        assert!(!s.update(&p, 3));
        assert_eq!(s.snapshot(), before);
    }

    #[test]
    fn reads_are_deterministic() {
        let s = SolUsdReferenceState::new();
        s.update(&upd(11_000_000_000, 100), 1);
        let a = s.read_micros(10, 120_000);
        for _ in 0..1000 {
            assert_eq!(s.read_micros(10, 120_000), a);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_exact_integer_price() {
        // 1 SOL (1e9 raw) against 200 USDC (200e6 raw) -> $200.00.
        assert_eq!(
            sol_usd_micros_from_reserves(1_000_000_000, 200_000_000),
            Some(200_000_000)
        );
        // 2 SOL against 300 USDC -> $150.00.
        assert_eq!(
            sol_usd_micros_from_reserves(2_000_000_000, 300_000_000),
            Some(150_000_000)
        );
    }

    #[test]
    fn zero_reserves_fail_closed() {
        assert_eq!(sol_usd_micros_from_reserves(0, 1), None);
        assert_eq!(sol_usd_micros_from_reserves(1, 0), None);
    }

    #[test]
    fn overflow_fails_closed() {
        assert_eq!(sol_usd_micros_from_reserves(1, u128::MAX), None);
    }

    #[test]
    fn reference_freshness() {
        let r =
            SolUsdReference::from_reserves(MarketKey([1u8; 32]), 1_000_000_000, 200_000_000, 1_000)
                .unwrap();
        assert_eq!(r.price_micros(), 200_000_000);
        assert!(r.is_fresh(1_000, 150));
        assert!(r.is_fresh(1_150, 150));
        assert!(!r.is_fresh(1_151, 150));
        // A reference from the future is never usable.
        assert!(!r.is_fresh(999, 150));
    }
}
