//! M4 — Strategy V1: a deterministic `OBSERVING -> QUALIFIED` decision.
//!
//! This module converts continuously maintained M3 market state into a
//! deterministic qualification decision. It **stops at qualification**: it does
//! not arm, construct, sign, submit, or manage anything.
//!
//! Design notes (see `docs/M4_STRATEGY_V1_REPORT.md` for the full rationale):
//!
//! * All decisions are pure functions of `MarketState` + `StrategyConfig` +
//!   the shard's current slot. The same state yields the same decision.
//! * USD thresholds are evaluated with a configured static quote-asset price
//!   (`sol_usd_price_micros`); there is no live oracle in scope. Without it M4
//!   is inactive (fail-closed: nothing qualifies).
//! * Only SOL-quoted markets are valued; other quote assets are rejected as
//!   `UNSUPPORTED_MARKET` rather than mis-valued.
//! * Unknown / stale / invalidated state, missing data, and unprovable safety
//!   conditions all fail closed.

use serde::Deserialize;

use crate::decode::Venue;
use crate::events::MarketKey;
use crate::market::{MarketState, ReserveState};
use crate::quote::{self, Side};

/// Strategy identity/version, used for deterministic dedup bookkeeping.
pub const STRATEGY_VERSION: u32 = 1;

/// Solana's target slot time is 400 ms -> 2.5 slots/second, represented exactly
/// as 5/2 so window arithmetic stays integer-only.
const SLOTS_PER_SECOND_NUM: u64 = 5;
const SLOTS_PER_SECOND_DEN: u64 = 2;

/// Quote asset ("SOL") has 9 decimals; its raw unit is the lamport.
const LAMPORTS_PER_SOL: u128 = 1_000_000_000;
/// USD thresholds are configured in whole dollars; compare in micros.
const MICROS_PER_USD: u128 = 1_000_000;
const BPS: u128 = 10_000;

/// Wrapped-SOL mint (`So11111111111111111111111111111111111111112`).
const SOL_MINT: [u8; 32] = [
    6, 155, 136, 87, 254, 171, 129, 132, 251, 104, 127, 99, 70, 24, 192, 53, 218, 196, 57, 220, 26,
    235, 59, 85, 152, 160, 240, 0, 0, 0, 0, 1,
];

/// Where the SOL/USD reference comes from.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SolUsdSource {
    /// Operator-supplied static price (`sol_usd_price_micros`). M4 is inactive
    /// while it is unset.
    #[default]
    Static,
    /// Pyth SOL/USD `PriceUpdateV2` account streamed over Yellowstone. M4 always
    /// evaluates and fails closed while the reference is absent/stale.
    PythYellowstone,
}

/// M4 strategy configuration. Locked values are the V1 specification; optional
/// values are deliberately `None` by default (no invented numbers).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StrategyConfig {
    /// Hard minimum liquidity in USD (inclusive).
    pub min_liquidity_usd: u64,
    /// Minimum market cap in USD (inclusive).
    pub min_market_cap_usd: u64,
    /// Maximum market cap in USD (inclusive).
    pub max_market_cap_usd: u64,
    /// Rolling volume window length, seconds.
    pub rolling_volume_window_seconds: u64,
    /// Minimum rolling-window volume in USD (inclusive).
    pub min_rolling_volume_usd: u64,
    /// SOL/USD price in micros (1e-6 USD). Required for USD gates; `None` (or
    /// 0) leaves M4 inactive. This is an operator input, not a strategy
    /// threshold; there is no live oracle in scope.
    pub sol_usd_price_micros: Option<u64>,
    /// SOL/USD reference source (see [`SolUsdSource`]).
    pub sol_usd_source: SolUsdSource,
    /// Maximum age of the local SOL/USD reference before it is treated as
    /// stale (milliseconds), measured on the monotonic receive clock. Derived
    /// from the Pyth sponsored feed's documented 55 s heartbeat: 120 s tolerates
    /// one missed heartbeat. Configurable; stale => fail closed.
    pub sol_usd_max_staleness_ms: u64,
    /// Maximum token age in seconds. `None` disables the freshness gate.
    /// Defaults to 900 s (15 minutes). This is a **maximum** age: a token must
    /// be at most this old to qualify. Enforced only when chain-proven creation
    /// is known; unknown creation fails closed while the gate is enabled.
    pub max_token_age_seconds: Option<u64>,
    /// Maximum MCAP drawdown from the running observed peak, in basis points.
    ///
    /// This is a **market qualification / anti-pump-retracement** filter, NOT a
    /// trade stop-loss. It rejects a market that already pumped and retraced
    /// substantially (e.g. $3k → $9k → $6k) even though its MCAP is still in
    /// range. Position stop-loss belongs to the later position/exit stages.
    /// `None` disables the gate (no project value has been selected).
    pub max_drawdown_bps: Option<u64>,
    /// Probe notional for executable-route checks, in raw quote units
    /// (lamports). Not a threshold; determines the quote size.
    pub probe_notional_lamports: u64,
    /// Maximum acceptable round-trip loss (buy then sell) in basis points of
    /// the probe notional. `None` disables this economic gate.
    pub max_roundtrip_loss_bps: Option<u64>,
    /// Maximum acceptable buy price impact in basis points versus the reference
    /// price. `None` disables this economic gate.
    pub max_price_impact_bps: Option<u64>,
    /// M5: maximum age of an armed execution context, in slots, measured from
    /// its first arm slot. An armed context must not outlive the reserve-
    /// freshness horizon it was armed against, so the default matches
    /// `market.reserve_stale_slots` (150 slots ≈ 60 s). Expiry fails closed.
    pub max_arm_age_slots: u64,
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            min_liquidity_usd: 2_000,
            min_market_cap_usd: 2_000,
            max_market_cap_usd: 10_000,
            rolling_volume_window_seconds: 300,
            min_rolling_volume_usd: 1_000,
            sol_usd_price_micros: None,
            sol_usd_source: SolUsdSource::Static,
            sol_usd_max_staleness_ms: 120_000,
            max_token_age_seconds: Some(900), // M4: 15 minutes
            max_drawdown_bps: None,
            probe_notional_lamports: 100_000_000, // 0.1 SOL
            max_roundtrip_loss_bps: None,
            max_price_impact_bps: None,
            max_arm_age_slots: 150,
        }
    }
}

impl StrategyConfig {
    /// M4 is active only when a usable quote price is configured.
    pub fn is_active(&self) -> bool {
        self.sol_usd_price_micros.is_some_and(|p| p > 0)
    }

    /// Rolling window length in slots (ceil of seconds * 2.5).
    pub fn window_slots(&self) -> u64 {
        let n = self
            .rolling_volume_window_seconds
            .saturating_mul(SLOTS_PER_SECOND_NUM);
        n.div_ceil(SLOTS_PER_SECOND_DEN)
    }

    /// Maximum token age in slots (ceil of `max_token_age_seconds` * 2.5).
    ///
    /// Slot-exact so the boundary is deterministic: exactly
    /// `max_age_slots` slots pass (inclusive), `max_age_slots + 1` fails.
    pub fn max_age_slots(&self) -> Option<u64> {
        self.max_token_age_seconds.map(|secs| {
            secs.saturating_mul(SLOTS_PER_SECOND_NUM)
                .div_ceil(SLOTS_PER_SECOND_DEN)
        })
    }
}

/// Deterministic rejection reasons.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RejectReason {
    UnsupportedMarket,
    TokenTooOld,
    InsufficientFreshnessData,
    AlreadyPumped,
    LowLiquidity,
    Low5mVolume,
    McapTooLow,
    McapTooHigh,
    UnsafeToken,
    UnsafeMarket,
    BuyUnavailable,
    SellUnavailable,
    BadExecutionEconomics,
    StateUnknown,
    StateStale,
    StateInvalidated,
    /// The SOL/USD reference required for USD valuation is absent or stale.
    ReferenceUnavailable,
    AlreadyConsumed,
}

impl RejectReason {
    pub fn as_str(self) -> &'static str {
        match self {
            RejectReason::UnsupportedMarket => "UNSUPPORTED_MARKET",
            RejectReason::TokenTooOld => "TOKEN_TOO_OLD",
            RejectReason::InsufficientFreshnessData => "INSUFFICIENT_FRESHNESS_DATA",
            RejectReason::AlreadyPumped => "ALREADY_PUMPED",
            RejectReason::LowLiquidity => "LOW_LIQUIDITY",
            RejectReason::Low5mVolume => "LOW_5M_VOLUME",
            RejectReason::McapTooLow => "MCAP_TOO_LOW",
            RejectReason::McapTooHigh => "MCAP_TOO_HIGH",
            RejectReason::UnsafeToken => "UNSAFE_TOKEN",
            RejectReason::UnsafeMarket => "UNSAFE_MARKET",
            RejectReason::BuyUnavailable => "BUY_UNAVAILABLE",
            RejectReason::SellUnavailable => "SELL_UNAVAILABLE",
            RejectReason::BadExecutionEconomics => "BAD_EXECUTION_ECONOMICS",
            RejectReason::StateUnknown => "STATE_UNKNOWN",
            RejectReason::StateStale => "STATE_STALE",
            RejectReason::StateInvalidated => "STATE_INVALIDATED",
            RejectReason::ReferenceUnavailable => "REFERENCE_UNAVAILABLE",
            RejectReason::AlreadyConsumed => "ALREADY_CONSUMED",
        }
    }
}

/// The M4 decision for one market at one instant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decision {
    Qualified,
    Rejected(RejectReason),
}

/// Convert a raw quote-unit amount (lamports) to USD micros.
fn quote_to_usd_micros(quote_raw: u128, sol_usd_micros: u64) -> Option<u128> {
    quote_raw
        .checked_mul(u128::from(sol_usd_micros))?
        .checked_div(LAMPORTS_PER_SOL)
}

/// USD threshold (whole dollars) expressed in micros.
fn usd_micros(usd: u64) -> u128 {
    u128::from(usd) * MICROS_PER_USD
}

/// Whether a market's quote asset can be valued as SOL.
fn quote_is_sol(quote_mint: Option<MarketKey>) -> bool {
    match quote_mint {
        None => true,
        Some(m) => {
            let b = m.as_bytes();
            b == &SOL_MINT || b == &[0u8; 32]
        }
    }
}

/// Provable deterministic structural safety checks. Returns a rejection when the
/// market is structurally unsafe/unprovable. Mint/freeze authority and Token-2022
/// extensions are **not** observable with the current ingestion (no mint-account
/// data) and are therefore documented limitations, not claimed guarantees.
fn safety_check(m: &MarketState) -> Option<RejectReason> {
    match m.venue {
        Some(Venue::PumpFun) => {
            if m.complete {
                // Bonding curve completed/migrated: not a curve trade.
                return Some(RejectReason::UnsafeMarket);
            }
            if m.token_total_supply == 0 {
                return Some(RejectReason::UnsafeToken);
            }
            if m.virtual_base_reserve == 0 || m.virtual_quote_reserve <= 0 {
                return Some(RejectReason::UnsafeMarket);
            }
        }
        Some(Venue::PumpSwap) => {
            if m.base_mint.is_none() || m.quote_mint.is_none() {
                return Some(RejectReason::UnsafeMarket);
            }
            if m.base_reserve == 0 || m.quote_reserve == 0 {
                return Some(RejectReason::UnsafeMarket);
            }
        }
        None => return Some(RejectReason::UnsupportedMarket),
    }
    None
}

/// Evaluate M4 for one market. Pure and deterministic.
///
/// Gate order (documented so the first failure reported is stable):
/// supported market -> state validity -> freshness -> structural safety ->
/// liquidity -> market cap -> anti-pump/dump -> rolling 5m volume -> buy route ->
/// sell route -> economics -> consumed dedup.
pub fn evaluate(
    market: &MarketState,
    current_slot: u64,
    stale_slots: u64,
    cfg: &StrategyConfig,
    sol_usd_price_micros: Option<u64>,
) -> Decision {
    use RejectReason::*;

    // USD gates need a live SOL/USD reference. Callers resolve it (static config
    // or the Pyth reference state) and fail closed when it is unavailable.
    let Some(price) = sol_usd_price_micros.filter(|p| *p > 0) else {
        return Decision::Rejected(ReferenceUnavailable);
    };

    // 1. Supported market (venue + valvable quote asset).
    let Some(venue) = market.venue else {
        return Decision::Rejected(UnsupportedMarket);
    };
    if !matches!(venue, Venue::PumpFun | Venue::PumpSwap) || !quote_is_sol(market.quote_mint) {
        return Decision::Rejected(UnsupportedMarket);
    }

    // 2. State validity (fail closed).
    match market.reserve_state(current_slot, stale_slots) {
        ReserveState::Known => {}
        ReserveState::Unknown => return Decision::Rejected(StateUnknown),
        ReserveState::Stale => return Decision::Rejected(StateStale),
        ReserveState::Invalidated => return Decision::Rejected(StateInvalidated),
    }

    // 3. Freshness — MAXIMUM age (only when configured; requires chain-proven
    // creation). Slot-exact: age <= max_age_slots passes (inclusive), one slot
    // over fails. Unknown creation fails closed.
    if let Some(max_age_slots) = cfg.max_age_slots() {
        if market.created_at_slot == 0 {
            return Decision::Rejected(InsufficientFreshnessData);
        }
        let age_slots = current_slot.saturating_sub(market.created_at_slot);
        if age_slots > max_age_slots {
            return Decision::Rejected(TokenTooOld);
        }
    }

    // 4. Deterministic structural safety (before economic gates so an
    // unsupported/unsafe market is reported as such).
    if let Some(reason) = safety_check(market) {
        return Decision::Rejected(reason);
    }

    // 5. Liquidity (inclusive minimum).
    let Some(liquidity_quote) = market.liquidity_quote() else {
        return Decision::Rejected(UnsafeMarket);
    };
    let Some(liq_usd) = quote_to_usd_micros(liquidity_quote, price) else {
        return Decision::Rejected(StateUnknown);
    };
    if liq_usd < usd_micros(cfg.min_liquidity_usd) {
        return Decision::Rejected(LowLiquidity);
    }

    // 6. Market cap (inclusive range).
    let Some(mcap_quote) = market.market_cap_quote() else {
        return Decision::Rejected(UnsafeMarket);
    };
    let Some(mcap_usd) = quote_to_usd_micros(mcap_quote, price) else {
        return Decision::Rejected(StateUnknown);
    };
    if mcap_usd < usd_micros(cfg.min_market_cap_usd) {
        return Decision::Rejected(McapTooLow);
    }
    if mcap_usd > usd_micros(cfg.max_market_cap_usd) {
        return Decision::Rejected(McapTooHigh);
    }

    // 7. Anti-pump / retracement **qualification** gate (only when configured).
    //
    // This is a MARKET QUALIFICATION filter, NOT a trade stop-loss:
    //   drawdown_bps = ((peak_mcap - current_mcap) * 10_000) / peak_mcap
    // where `peak_mcap` is the running maximum Neurone has observed for this
    // market (including the current observation). A new high therefore yields
    // drawdown 0; a retracement yields the true decline from the observed peak.
    // Integer-only; overflow or unestablished extrema fail closed.
    if let Some(max_dd) = cfg.max_drawdown_bps {
        if market.created_at_slot == 0 || market.peak_mcap_quote == 0 {
            return Decision::Rejected(InsufficientFreshnessData);
        }
        let peak = market.peak_mcap_quote;
        let current = mcap_quote.min(peak); // current >= peak => no drawdown
        let Some(dd_bps) = (peak - current).checked_mul(BPS).map(|n| n / peak) else {
            return Decision::Rejected(InsufficientFreshnessData);
        };
        if dd_bps > u128::from(max_dd) {
            return Decision::Rejected(AlreadyPumped);
        }
    }

    // 8. Rolling 5-minute volume (inclusive minimum).
    let from_slot = current_slot.saturating_sub(cfg.window_slots());
    let vol_quote = u128::from(market.rolling_quote_volume(from_slot));
    let Some(vol_usd) = quote_to_usd_micros(vol_quote, price) else {
        return Decision::Rejected(StateUnknown);
    };
    if vol_usd < usd_micros(cfg.min_rolling_volume_usd) {
        return Decision::Rejected(Low5mVolume);
    }

    // 9. Executable buy route from current state.
    let fee_bps = market.last_fee_bps.unwrap_or(0);
    let buy = match quote::quote(
        market,
        Side::Buy,
        cfg.probe_notional_lamports,
        fee_bps,
        current_slot,
        stale_slots,
    ) {
        Ok(q) if q.net_output > 0 => q,
        _ => return Decision::Rejected(BuyUnavailable),
    };

    // 10. Executable reverse sell route (sell what the buy would yield).
    let Ok(tokens) = u64::try_from(buy.net_output) else {
        return Decision::Rejected(SellUnavailable);
    };
    let sell = match quote::quote(
        market,
        Side::Sell,
        tokens,
        fee_bps,
        current_slot,
        stale_slots,
    ) {
        Ok(q) if q.net_output > 0 => q,
        _ => return Decision::Rejected(SellUnavailable),
    };

    // 11. Execution economics (only the configured bounds).
    if let Some(max_impact) = cfg.max_price_impact_bps {
        let (Some(eff), Some(reference)) = (buy.effective_price, market.reference_price_raw())
        else {
            return Decision::Rejected(BadExecutionEconomics);
        };
        // impact_bps = (eff / reference - 1) * 10_000, integer.
        let num = eff
            .num
            .checked_mul(reference.den)
            .and_then(|x| x.checked_mul(BPS));
        let den = eff.den.checked_mul(reference.num);
        match (num, den) {
            (Some(num), Some(den)) if den > 0 => {
                let impact_bps = (num / den).saturating_sub(BPS);
                if impact_bps > u128::from(max_impact) {
                    return Decision::Rejected(BadExecutionEconomics);
                }
            }
            _ => return Decision::Rejected(BadExecutionEconomics),
        }
    }
    if let Some(max_loss) = cfg.max_roundtrip_loss_bps {
        let input = u128::from(cfg.probe_notional_lamports);
        if input == 0 {
            return Decision::Rejected(BadExecutionEconomics);
        }
        let proceeds_bps = sell.net_output.saturating_mul(BPS) / input;
        let loss_bps = BPS.saturating_sub(proceeds_bps);
        if loss_bps > u128::from(max_loss) {
            return Decision::Rejected(BadExecutionEconomics);
        }
    }

    // 12. Dedup: an opportunity already consumed at this strategy version cannot
    // re-qualify.
    if market.consumed_version != 0 && market.consumed_version == market.strategy_version {
        return Decision::Rejected(AlreadyConsumed);
    }

    Decision::Qualified
}
