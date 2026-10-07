//! Exact protocol quote engine.
//!
//! ```text
//! OBSERVED MARKET STATE  ->  EXACT QUOTE ENGINE  ->  future qualification/arming
//! ```
//!
//! Every computation is **exact integer math** in raw on-chain units
//! (`u128` with checked arithmetic), with explicit rounding direction and fees.
//! There is no floating point on this path. Quotes never touch the network and
//! never build or send a transaction.
//!
//! ## Venue mechanics (verified against real mainnet events)
//!
//! * **pump.swap** — constant product on the pool reserves.
//!   * buy: `out_base = floor(base * q_net / (quote + q_net))`, where
//!     `q_net` is the input after fees.
//!   * sell: `gross_quote = floor(quote * base_in / (base + base_in))`, then
//!     fees are deducted from the quote output.
//!   * The sell formula was verified **exactly** against real `SellEvent`s.
//! * **pump.fun** — Uniswap-V2-style virtual reserves.
//!   * sell: `gross_quote = floor(v_quote * base_in / (v_base + base_in))`,
//!     verified **exactly** against real `TradeEvent`s.
//!   * buy: same constant-product form on the net input. Exact integer parity
//!     for on-chain *buys* could not be established from public sources; see
//!     `docs/MILESTONE_2_1_REPORT.md` (residual ≤ ~1 lamport's worth).

use crate::decode::Venue;
use crate::events::MarketKey;
use crate::market::{MarketState, Ratio, ReserveState};

/// Trade direction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Buy,
    Sell,
}

/// Why a quote could not be produced. Every variant is deterministic.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QuoteError {
    /// No authoritative reserves observed yet.
    ReservesUnknown,
    /// Reserves observed but not fresh enough.
    ReservesStale,
    /// Market venue is not one we can quote.
    UnsupportedVenue,
    /// Zero input amount.
    ZeroInput,
    /// A reserve side is zero.
    ZeroReserves,
    /// Input too large relative to reserves to produce any output.
    InsufficientLiquidity,
    /// Fee basis points out of range (> 10000).
    InvalidFee,
    /// Integer overflow while computing.
    Overflow,
}

/// A quote result. All amounts are raw integer units (base token units /
/// quote units). `gross_output` is before fees, `net_output` after.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Quote {
    pub venue: Venue,
    pub side: Side,
    pub input_amount: u64,
    pub gross_output: u128,
    pub fee_amount: u128,
    pub net_output: u128,
    /// Effective price: raw quote per raw base for the whole trade.
    pub effective_price: Option<Ratio>,
    pub valid: bool,
}

const BPS: u128 = 10_000;

/// Compute an exact quote against the market's current reserves.
///
/// `current_slot` + `stale_slots` gate on reserve freshness; `fee_bps` is the
/// total fee in basis points for the venue/side.
pub fn quote(
    market: &MarketState,
    side: Side,
    input_amount: u64,
    fee_bps: u64,
    current_slot: u64,
    stale_slots: u64,
) -> Result<Quote, QuoteError> {
    match market.reserve_state(current_slot, stale_slots) {
        ReserveState::Unknown => return Err(QuoteError::ReservesUnknown),
        ReserveState::Stale => return Err(QuoteError::ReservesStale),
        ReserveState::Known => {}
    }
    if fee_bps > 10_000 {
        return Err(QuoteError::InvalidFee);
    }
    if input_amount == 0 {
        return Err(QuoteError::ZeroInput);
    }
    let venue = market.venue.ok_or(QuoteError::UnsupportedVenue)?;
    match venue {
        Venue::PumpSwap => quote_pumpswap(market, side, input_amount, fee_bps),
        Venue::PumpFun => quote_pumpfun(market, side, input_amount, fee_bps),
    }
}

/// Fee deduction helper: `net = amount - amount*fee_bps/10000` (floor).
/// Returns `(net, fee)`; `fee = amount - net`.
fn apply_fee(amount: u128, fee_bps: u64) -> Option<(u128, u128)> {
    let net = amount.checked_mul(BPS - u128::from(fee_bps))? / BPS;
    Some((net, amount.checked_sub(net)?))
}

/// Constant-product output for a given input: `floor(reserve_out * net_in /
/// (reserve_in + net_in))`. Returns `None` on overflow.
fn k_out(reserve_in: u128, reserve_out: u128, net_in: u128) -> Option<u128> {
    let num = reserve_out.checked_mul(net_in)?;
    let den = reserve_in.checked_add(net_in)?;
    if den == 0 {
        return None;
    }
    Some(num / den)
}

fn quote_pumpswap(
    market: &MarketState,
    side: Side,
    input_amount: u64,
    fee_bps: u64,
) -> Result<Quote, QuoteError> {
    let base = market.base_reserve;
    let quote_res = market.quote_reserve;
    if base == 0 || quote_res == 0 {
        return Err(QuoteError::ZeroReserves);
    }
    let input = u128::from(input_amount);
    match side {
        // Buy base with quote: fees come out of the quote input.
        Side::Buy => {
            let (net_in, fee) = apply_fee(input, fee_bps).ok_or(QuoteError::Overflow)?;
            let gross = k_out(quote_res, base, net_in).ok_or(QuoteError::Overflow)?;
            if gross == 0 {
                return Err(QuoteError::InsufficientLiquidity);
            }
            // net_output == gross (no token-side fee); fee is in quote units.
            Ok(Quote {
                venue: Venue::PumpSwap,
                side,
                input_amount,
                gross_output: gross,
                fee_amount: fee,
                net_output: gross,
                effective_price: Ratio::new(input, gross),
                valid: true,
            })
        }
        // Sell base for quote: fees come out of the quote output.
        Side::Sell => {
            let gross = k_out(base, quote_res, input).ok_or(QuoteError::Overflow)?;
            if gross == 0 {
                return Err(QuoteError::InsufficientLiquidity);
            }
            let (net, fee) = apply_fee(gross, fee_bps).ok_or(QuoteError::Overflow)?;
            Ok(Quote {
                venue: Venue::PumpSwap,
                side,
                input_amount,
                gross_output: gross,
                fee_amount: fee,
                net_output: net,
                effective_price: Ratio::new(net, input),
                valid: true,
            })
        }
    }
}

fn quote_pumpfun(
    market: &MarketState,
    side: Side,
    input_amount: u64,
    fee_bps: u64,
) -> Result<Quote, QuoteError> {
    // Prefer synthetic (virtual) reserves, matching the curve mechanics.
    let v_base = if market.virtual_base_reserve > 0 {
        market.virtual_base_reserve
    } else {
        market.base_reserve
    };
    let v_quote = if market.virtual_quote_reserve > 0 {
        market.virtual_quote_reserve as u128
    } else {
        market.quote_reserve
    };
    if v_base == 0 || v_quote == 0 {
        return Err(QuoteError::ZeroReserves);
    }
    let input = u128::from(input_amount);
    match side {
        Side::Buy => {
            let (net_in, fee) = apply_fee(input, fee_bps).ok_or(QuoteError::Overflow)?;
            let tokens = k_out(v_quote, v_base, net_in).ok_or(QuoteError::Overflow)?;
            if tokens == 0 {
                return Err(QuoteError::InsufficientLiquidity);
            }
            Ok(Quote {
                venue: Venue::PumpFun,
                side,
                input_amount,
                gross_output: tokens,
                fee_amount: fee,
                net_output: tokens,
                effective_price: Ratio::new(input, tokens),
                valid: true,
            })
        }
        Side::Sell => {
            let gross = k_out(v_base, v_quote, input).ok_or(QuoteError::Overflow)?;
            if gross == 0 {
                return Err(QuoteError::InsufficientLiquidity);
            }
            let (net, fee) = apply_fee(gross, fee_bps).ok_or(QuoteError::Overflow)?;
            Ok(Quote {
                venue: Venue::PumpFun,
                side,
                input_amount,
                gross_output: gross,
                fee_amount: fee,
                net_output: net,
                effective_price: Ratio::new(net, input),
                valid: true,
            })
        }
    }
}

/// Convenience: market key helper for callers that only need identity.
pub fn market_key(market: &MarketState) -> MarketKey {
    market.key
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::Venue;

    fn market(venue: Venue, base: u128, quote: u128) -> MarketState {
        let mut m = MarketState::new(MarketKey([1u8; 32]), 0);
        m.venue = Some(venue);
        m.base_reserve = base;
        m.quote_reserve = quote;
        m.reserves_known = true;
        m.last_reserve_slot = 100;
        if venue == Venue::PumpFun {
            m.virtual_base_reserve = base;
            m.virtual_quote_reserve = quote as i128;
        }
        m
    }

    #[test]
    fn zero_input_and_unknown_reserves_are_rejected() {
        let m = market(Venue::PumpSwap, 1_000_000, 5_000_000);
        assert_eq!(
            quote(&m, Side::Buy, 0, 25, 100, 10),
            Err(QuoteError::ZeroInput)
        );
        let mut unknown = MarketState::new(MarketKey([2u8; 32]), 0);
        unknown.venue = Some(Venue::PumpSwap);
        assert_eq!(
            quote(&unknown, Side::Buy, 100, 25, 100, 10),
            Err(QuoteError::ReservesUnknown)
        );
    }

    #[test]
    fn zero_reserves_are_rejected() {
        let m = market(Venue::PumpSwap, 0, 5_000_000);
        assert_eq!(
            quote(&m, Side::Buy, 100, 25, 100, 10),
            Err(QuoteError::ZeroReserves)
        );
    }

    #[test]
    fn pump_swap_buy_is_exact_constant_product_with_quote_fee() {
        // base=1_000_000, quote=5_000_000, fee 25 bps
        let m = market(Venue::PumpSwap, 1_000_000, 5_000_000);
        let q = quote(&m, Side::Buy, 1_500, 25, 100, 10).unwrap();
        // net_in = 1500*9975/10000 = 1496 ; out = 1_000_000*1496/(5_000_000+1496) = 299
        assert_eq!(q.fee_amount, 4);
        assert_eq!(q.net_output, 1_000_000 * 1496 / (5_000_000 + 1496));
        assert_eq!(q.net_output, 299);
        assert_eq!(q.effective_price.unwrap(), Ratio::new(1500, 299).unwrap());
    }

    #[test]
    fn pump_swap_sell_fee_comes_from_output() {
        let m = market(Venue::PumpSwap, 1_000_000, 5_000_000);
        let q = quote(&m, Side::Sell, 1_000, 25, 100, 10).unwrap();
        let gross = 5_000_000u128 * 1_000 / (1_000_000 + 1_000);
        assert_eq!(q.gross_output, gross);
        assert_eq!(q.net_output, gross * 9975 / 10_000);
        assert_eq!(q.fee_amount, gross - q.net_output);
    }

    #[test]
    fn pump_fun_sell_matches_reserve_formula() {
        let m = market(Venue::PumpFun, 1_073_000_000_000_000, 30_000_000_000);
        let q = quote(&m, Side::Sell, 1_000_000_000, 100, 100, 10).unwrap();
        let gross = 30_000_000_000u128 * 1_000_000_000 / (1_073_000_000_000_000 + 1_000_000_000);
        assert_eq!(q.gross_output, gross);
        assert_eq!(q.net_output, gross * 9900 / 10_000);
    }

    #[test]
    fn stale_reserves_are_rejected() {
        let m = market(Venue::PumpSwap, 1_000_000, 5_000_000);
        assert_eq!(
            quote(&m, Side::Buy, 100, 25, 200, 10),
            Err(QuoteError::ReservesStale)
        );
        // Exactly at the boundary is still fresh.
        assert!(quote(&m, Side::Buy, 100, 25, 110, 10).is_ok());
    }

    #[test]
    fn invalid_fee_and_overflow_are_rejected() {
        let m = market(Venue::PumpSwap, 1_000_000, 5_000_000);
        assert_eq!(
            quote(&m, Side::Buy, 100, 10_001, 100, 10),
            Err(QuoteError::InvalidFee)
        );
        // Huge input into tiny reserves still yields a valid (huge) quote but
        // must not overflow; a directly overflowing multiplication is caught.
        let big = market(Venue::PumpSwap, u128::MAX / 2, u128::MAX / 2);
        assert_eq!(
            quote(&big, Side::Buy, u64::MAX, 0, 100, 10),
            Err(QuoteError::Overflow)
        );
    }

    #[test]
    fn insufficient_liquidity_is_deterministic() {
        // Tiny reserves: a 1-base input to a 1-base pool with 0 fee yields 0 out.
        let m = market(Venue::PumpSwap, 1, 1);
        assert_eq!(
            quote(&m, Side::Sell, 1, 0, 100, 10),
            Err(QuoteError::InsufficientLiquidity)
        );
    }
}
