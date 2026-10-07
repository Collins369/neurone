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

use crate::decode::DecodedSwap;
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
    /// Instruction name not recognized (refuse to guess).
    UnsupportedInstruction,
    /// Trade pre-state could not be corroborated (refuse to quote).
    UnsupportedState,
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

/// Public constant-product output helper: `floor(reserve_out·input /
/// (reserve_in + input))`. Returns `None` on overflow or zero denominator.
pub fn constant_product(reserve_in: u128, reserve_out: u128, input: u128) -> Option<u128> {
    k_out(reserve_in, reserve_out, input)
}

// ---------------------------------------------------------------------------
// Official SDK fee arithmetic (client-side quoting)
//
// Source: `@pump-fun/pump-sdk` v3.0.0 (`src/bondingCurve.ts`, `src/fees.ts`).
// These map a **user-specified gross amount** to the bonding-curve value. They
// are distinct from interpreting a `TradeEvent`, whose `sol_amount` is already
// a curve-level (post-fee) value — see the M3.2A report.
// ---------------------------------------------------------------------------

/// Official fee: `ceil(amount * bps / 10000)`.
pub fn fee_ceil(amount: u128, bps: u64) -> Option<u128> {
    amount
        .checked_mul(u128::from(bps))
        .map(|v| v.div_ceil(10_000))
}

/// Official BUY (SOL/quote in): the curve input for a user gross amount.
///
/// `total_bps` is the protocol rate plus the creator rate when the creator is
/// set (never otherwise).
pub fn buy_input_from_gross(gross: u64, total_bps: u64) -> Option<u128> {
    let g = u128::from(gross).checked_sub(1)?;
    Some(g.checked_mul(10_000)? / (u128::from(total_bps) + 10_000))
}

/// Official token-target BUY: `floor(x·vq/(vt−x)) + 1` (the `+1` is explicit,
/// not a generic ceiling).
pub fn token_target_sol_cost(vt: u128, vq: u128, x: u128) -> Option<u128> {
    let den = vt.checked_sub(x)?;
    if den == 0 {
        return None;
    }
    vq.checked_mul(x)?.checked_div(den)?.checked_add(1)
}

/// Official SELL: user net = `gross − fee(gross, protocol) − fee(gross, creator)`
/// with the creator term omitted when no creator is set.
pub fn sell_net_from_gross(
    gross: u128,
    protocol_bps: u64,
    creator_bps: Option<u64>,
) -> Option<u128> {
    let mut net = gross.checked_sub(fee_ceil(gross, protocol_bps)?)?;
    if let Some(bps) = creator_bps {
        net = net.checked_sub(fee_ceil(gross, bps)?)?;
    }
    Some(net)
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

// ---------------------------------------------------------------------------
// Reconciled protocol-parity formulas (M3)
//
// These operate on a decoded swap's own trade-time pre-state, so they can be
// validated directly against the on-chain result recorded in the same event.
// Semantics established by the docs/investigation reconciliation:
//   * pump.fun events store POST-trade reserves  -> pre = post - delta
//   * pump.swap events store PRE-trade reserves  -> pre = event reserves
//   * pump.swap effective quote = raw_quote + signed virtual_quote_reserves
// ---------------------------------------------------------------------------

/// Protocol instruction that produced a swap.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Instruction {
    /// Sell base tokens for quote.
    Sell,
    /// Exact quote-in buy (pump.fun `buy_exact_sol_in`/`buy_exact_quote_in`,
    /// pump.swap `buy_exact_quote_in`).
    BuyExactIn,
    /// Token-target buy (`buy` / `buy_v2`): base amount specified.
    BuyTokenTarget,
}

impl Instruction {
    pub fn as_str(self) -> &'static str {
        match self {
            Instruction::Sell => "sell",
            Instruction::BuyExactIn => "buy_exact_in",
            Instruction::BuyTokenTarget => "buy_token_target",
        }
    }
}

/// Classify a decoded swap from its venue + `ix_name` (+ side).
pub fn classify(swap: &DecodedSwap) -> Result<Instruction, QuoteError> {
    match swap.venue {
        Venue::PumpFun => match swap.ix_name.as_deref() {
            Some("sell") => Ok(Instruction::Sell),
            Some("sell_v2") => Ok(Instruction::Sell),
            Some("buy_exact_sol_in")
            | Some("buy_exact_quote_in")
            | Some("buy_exact_quote_in_v2") => Ok(Instruction::BuyExactIn),
            Some("buy") | Some("buy_v2") => Ok(Instruction::BuyTokenTarget),
            _ => Err(QuoteError::UnsupportedInstruction),
        },
        Venue::PumpSwap => {
            if !swap.is_buy {
                Ok(Instruction::Sell) // SellEvent carries no ix_name
            } else {
                match swap.ix_name.as_deref() {
                    Some("buy_exact_quote_in") | Some("buy_exact_quote_in_v2") => {
                        Ok(Instruction::BuyExactIn)
                    }
                    Some("buy") | Some("buy_v2") => Ok(Instruction::BuyTokenTarget),
                    _ => Err(QuoteError::UnsupportedInstruction),
                }
            }
        }
    }
}

fn ceil_div(a: u128, b: u128) -> Option<u128> {
    (b != 0).then(|| a.div_ceil(b))
}

/// A decoded swap's trade-time pre-state plus the observed result.
#[derive(Clone, Copy, Debug)]
pub struct SwapPreState {
    pub instruction: Instruction,
    /// Pre-trade base reserve (raw).
    pub pre_base: u128,
    /// Pre-trade quote reserve (raw or effective).
    pub pre_quote: u128,
    /// Quote amount the user supplied (exact-in) or the pool took in.
    pub quote_input: u128,
    /// Base amount the trade moved (the observed result for buys).
    pub base_result: u128,
    /// Quote amount the trade moved (the observed result for sells).
    pub quote_result: u128,
}

/// Derive the trade-time pre-state from a decoded swap.
pub fn swap_pre_state(swap: &DecodedSwap) -> Result<SwapPreState, QuoteError> {
    let instruction = classify(swap)?;
    match swap.venue {
        Venue::PumpFun => {
            // Event reserves are POST-trade; reverse the trade to get pre-state.
            let vb = swap.virtual_base_reserve.ok_or(QuoteError::ZeroReserves)? as u128;
            let vq = swap.virtual_quote_reserve.ok_or(QuoteError::ZeroReserves)?;
            let delta_q = i128::from(swap.quote_amount);
            let delta_b = i128::from(swap.base_amount);
            let (pre_b, pre_q) = if swap.is_buy {
                (vb as i128 + delta_b, vq - delta_q)
            } else {
                (vb as i128 - delta_b, vq + delta_q)
            };
            if pre_b <= 0 || pre_q <= 0 {
                return Err(QuoteError::ZeroReserves);
            }
            Ok(SwapPreState {
                instruction,
                pre_base: pre_b as u128,
                pre_quote: pre_q as u128,
                quote_input: u128::from(swap.quote_amount),
                base_result: u128::from(swap.base_amount),
                quote_result: u128::from(swap.quote_amount),
            })
        }
        Venue::PumpSwap => {
            // Event reserves are PRE-trade. Effective quote = raw + signed virt.
            let pre_base = u128::from(swap.base_reserve.ok_or(QuoteError::ZeroReserves)?);
            let raw_quote = u128::from(swap.quote_reserve.ok_or(QuoteError::ZeroReserves)?);
            let virt = swap.virtual_quote_reserve.unwrap_or(0);
            let eff = (raw_quote as i128)
                .checked_add(virt)
                .ok_or(QuoteError::Overflow)?;
            if eff < 0 {
                return Err(QuoteError::ZeroReserves);
            }
            Ok(SwapPreState {
                instruction,
                pre_base,
                pre_quote: eff as u128,
                // Exact-quote-in uses the *user* quote amount (net); sells and
                // token-target buys use the pool-side quote movement.
                quote_input: if instruction == Instruction::BuyExactIn {
                    u128::from(swap.user_quote_amount)
                } else {
                    u128::from(swap.quote_amount)
                },
                base_result: u128::from(swap.base_amount),
                quote_result: u128::from(swap.quote_amount),
            })
        }
    }
}

/// Predicted **pool-level** output for a decoded swap.
///
/// For sells this is the gross quote out; for buys it is the base out — the
/// same quantity the event records, so it can be compared for exact parity.
pub fn predict_swap(swap: &DecodedSwap) -> Result<u128, QuoteError> {
    let s = swap_pre_state(swap)?;
    let (pb, pq) = (s.pre_base, s.pre_quote);
    if pb == 0 || pq == 0 {
        return Err(QuoteError::ZeroReserves);
    }
    match s.instruction {
        // Sell: base in -> quote out (priced against effective/virtual quote).
        Instruction::Sell => k_out(pb, pq, s.base_result).ok_or(QuoteError::Overflow),
        Instruction::BuyExactIn => {
            let net = s.quote_input.checked_sub(1).ok_or(QuoteError::ZeroInput)?;
            let den = pq.checked_add(net).ok_or(QuoteError::Overflow)?;
            if den == 0 {
                return Err(QuoteError::ZeroReserves);
            }
            Ok(pb.checked_mul(net).ok_or(QuoteError::Overflow)? / den)
        }
        Instruction::BuyTokenTarget => {
            // pool took `quote_input` in, base out follows the same CP form.
            k_out(pq, pb, s.quote_input).ok_or(QuoteError::Overflow)
        }
    }
}

/// Predicted quote required for a token-target buy (`buy`): the minimal quote
/// such that the constant product yields `base_target` tokens (rounded up).
pub fn predict_token_target_quote(swap: &DecodedSwap) -> Result<u128, QuoteError> {
    let s = swap_pre_state(swap)?;
    let (pb, pq) = (s.pre_base, s.pre_quote);
    let target = s.base_result;
    if target >= pb {
        return Err(QuoteError::InsufficientLiquidity);
    }
    ceil_div(pq * target, pb - target).ok_or(QuoteError::Overflow)
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
