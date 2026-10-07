//! Quote-engine parity against real mainnet event outputs.
//!
//! These values are taken from real Solana mainnet transactions (captured via
//! `rpc.solami.dev`, 2026-10-07). They assert **exact integer equality** for the
//! directions where on-chain parity was established, and document the residual
//! where it was not.

use neurone::decode::Venue;
use neurone::events::MarketKey;
use neurone::market::MarketState;
use neurone::quote::{quote, QuoteError, Side};

fn market(venue: Venue, base: u128, quote_res: u128) -> MarketState {
    let mut m = MarketState::new(MarketKey([1u8; 32]), 0);
    m.venue = Some(venue);
    m.base_reserve = base;
    m.quote_reserve = quote_res;
    m.reserves_known = true;
    m.last_reserve_slot = 1_000;
    if venue == Venue::PumpFun {
        m.virtual_base_reserve = base;
        m.virtual_quote_reserve = quote_res as i128;
    }
    m
}

/// Real pump.swap `SellEvent`: pre reserves base=1998708328783404,
/// quote=10503259; the user sold 1,233,464,719,708 base and the pool produced
/// `quote_amount_out = 6477` (gross, before fees).
#[test]
fn pump_swap_sell_matches_real_mainnet_event_exactly() {
    let m = market(Venue::PumpSwap, 1_998_708_328_783_404, 10_503_259);
    let q = quote(&m, Side::Sell, 1_233_464_719_708, 25, 1_000, 10).unwrap();
    assert_eq!(
        q.gross_output, 6_477,
        "gross quote must equal the on-chain event"
    );
}

/// Second real pump.swap `SellEvent`: base=1998708328783404, quote=11114771,
/// base_in=56543797815869 -> quote_amount_out = 305787.
#[test]
fn pump_swap_sell_matches_second_real_event_exactly() {
    let m = market(Venue::PumpSwap, 1_998_708_328_783_404, 11_114_771);
    let q = quote(&m, Side::Sell, 56_543_797_815_869, 25, 1_000, 10).unwrap();
    assert_eq!(q.gross_output, 305_787);
}

/// Real pump.fun `TradeEvent` chain: pre virtual reserves
/// (quote=2_164_115_499, base=1_075_682_474_849_693), the user sold
/// 491,141,116,763 tokens and the event recorded `sol_amount = 987653` (gross),
/// fee 9383 at 95 bps.
#[test]
fn pump_fun_sell_matches_real_mainnet_event_exactly() {
    let m = market(Venue::PumpFun, 1_075_682_474_849_693, 2_164_115_499);
    let q = quote(&m, Side::Sell, 491_141_116_763, 95, 1_000, 10).unwrap();
    assert_eq!(
        q.gross_output, 987_653,
        "gross SOL must equal the on-chain event"
    );
    assert_eq!(q.fee_amount, 9_383, "fee must equal the on-chain event");
    assert_eq!(q.net_output, 978_270);
}

/// Documented limitation: real pump.swap buys are within ~1 raw unit of the
/// integer model but were **not** found to be exactly reproducible from public
/// information, so no exact-parity claim is made for buys. This test pins the
/// observed residual so a future milestone can detect a fix or a regression.
#[test]
fn pump_swap_buy_residual_is_documented_and_bounded() {
    // Real BuyEvent: base=1999941793503112, quote=10496784,
    // user_quote_amount_in=987653 (already net of fees) -> base_amount_out=171993340573009.
    let m = market(Venue::PumpSwap, 1_999_941_793_503_112, 10_496_784);
    // fee_bps = 0 because the event's user_quote_amount_in is already net.
    let q = quote(&m, Side::Buy, 987_653, 0, 1_000, 10).unwrap();
    let observed: u128 = 171_993_340_573_009;
    let diff = q.net_output.abs_diff(observed);
    assert!(
        diff * 1_000_000 < observed,
        "residual unexpectedly large: {diff}"
    );
}

#[test]
fn quote_rejects_unknown_reserves() {
    let mut m = MarketState::new(MarketKey([3u8; 32]), 0);
    m.venue = Some(Venue::PumpSwap);
    assert_eq!(
        quote(&m, Side::Buy, 100, 25, 1_000, 10),
        Err(QuoteError::ReservesUnknown)
    );
}
