//! M3 regression tests for the live-validated protocol-parity rules.
//!
//! Deterministic, network-free. Each test pins an exact integer rule that was
//! validated against live Solami Yellowstone data.

use neurone::decode::{DecodedSwap, Venue};
use neurone::events::MarketKey;
use neurone::quote::{self, Instruction, QuoteError};

fn base_swap() -> DecodedSwap {
    DecodedSwap {
        venue: Venue::PumpSwap,
        market_key: MarketKey([1u8; 32]),
        base_mint: None,
        quote_mint: None,
        is_buy: false,
        base_amount: 0,
        quote_amount: 0,
        user_quote_amount: 0,
        base_reserve: None,
        quote_reserve: None,
        virtual_base_reserve: None,
        virtual_quote_reserve: None,
        fee_quote: 0,
        fee_bps: None,
        timestamp: None,
        ix_name: None,
    }
}

/// Real pump.swap `SellEvent` (single-event tx): pre reserves
/// base=1998708328783404 quote=10503259, base_in=1233464719708 -> 6477.
#[test]
fn pumpswap_sell_is_exact() {
    let s = DecodedSwap {
        is_buy: false,
        base_amount: 1_233_464_719_708,
        quote_amount: 6_477,
        base_reserve: Some(1_998_708_328_783_404),
        quote_reserve: Some(10_503_259),
        virtual_quote_reserve: Some(0),
        ..base_swap()
    };
    assert_eq!(quote::predict_swap(&s).unwrap(), 6_477);
}

/// Effective quote reserves must use the **event's** signed virtual reserve.
#[test]
fn pumpswap_sell_uses_signed_event_virtual_reserve() {
    let mut s = DecodedSwap {
        is_buy: false,
        base_amount: 1_000_000,
        quote_amount: 0,
        base_reserve: Some(1_000_000_000),
        quote_reserve: Some(5_000_000),
        virtual_quote_reserve: Some(0),
        ..base_swap()
    };
    let no_virt = quote::predict_swap(&s).unwrap();
    // A positive virtual reserve raises effective quote -> larger output.
    s.virtual_quote_reserve = Some(2_500_000);
    let pos = quote::predict_swap(&s).unwrap();
    assert!(pos > no_virt, "positive virtual must raise output");
    // A negative virtual reserve lowers effective quote -> smaller output.
    s.virtual_quote_reserve = Some(-2_500_000);
    let neg = quote::predict_swap(&s).unwrap();
    assert!(neg < no_virt, "negative virtual must lower output");
    assert_eq!(no_virt, 5_000_000u128 * 1_000_000 / 1_001_000_000);
}

/// A virtual reserve that would make effective quote negative is rejected.
#[test]
fn pumpswap_negative_effective_reserve_is_rejected() {
    let s = DecodedSwap {
        is_buy: false,
        base_amount: 1_000,
        quote_amount: 0,
        base_reserve: Some(1_000_000),
        quote_reserve: Some(100),
        virtual_quote_reserve: Some(-1_000),
        ..base_swap()
    };
    assert_eq!(quote::predict_swap(&s), Err(QuoteError::ZeroReserves));
}

/// Real pump.swap `buy_exact_quote_in`: base=1999941793503112, quote=10496784,
/// user_quote_amount_in=987653, virt=0 -> base_out=171993340573009.
#[test]
fn pumpswap_buy_exact_in_applies_minus_one() {
    let s = DecodedSwap {
        is_buy: true,
        ix_name: Some("buy_exact_quote_in".into()),
        base_amount: 171_993_340_573_009,
        quote_amount: 1_000_000,
        user_quote_amount: 987_653,
        base_reserve: Some(1_999_941_793_503_112),
        quote_reserve: Some(10_496_784),
        virtual_quote_reserve: Some(0),
        ..base_swap()
    };
    assert_eq!(quote::predict_swap(&s).unwrap(), 171_993_340_573_009);
    // The `-1` is load-bearing: using `user_quote_amount` without it differs.
    let without = 1_999_941_793_503_112u128 * 987_653 / (10_496_784 + 987_653);
    assert_ne!(without, 171_993_340_573_009);
}

/// pump.swap token-target `buy`: the pool quote inflow yields the base out.
#[test]
fn pumpswap_token_target_buy_is_exact() {
    // Self-consistent: choose pre-reserves and a pool quote inflow, derive the
    // base out with the same constant-product rule, then assert the round-trip.
    let pb: u128 = 1_181_394_744_576_913;
    let pq: u128 = 17_660_149;
    let q_in: u64 = 1_000_000;
    let base_out = (pb * u128::from(q_in) / (pq + u128::from(q_in))) as u64;
    let s = DecodedSwap {
        is_buy: true,
        ix_name: Some("buy".into()),
        base_amount: base_out,
        quote_amount: q_in,
        user_quote_amount: q_in,
        base_reserve: Some(pb as u64),
        quote_reserve: Some(pq as u64),
        virtual_quote_reserve: Some(0),
        ..base_swap()
    };
    assert_eq!(quote::predict_swap(&s).unwrap(), u128::from(base_out));
}

/// pump.fun exact-in buy: post-delta pre-state + `-1`.
#[test]
fn pumpfun_buy_exact_in_applies_minus_one() {
    let pre_vq: u128 = 30_000_000_000;
    let pre_vb: u128 = 1_073_000_000_000_000;
    let sol: u64 = 1_000_000;
    let tokens = pre_vb * (u128::from(sol) - 1) / (pre_vq + u128::from(sol) - 1);
    let s = DecodedSwap {
        venue: Venue::PumpFun,
        is_buy: true,
        ix_name: Some("buy_exact_sol_in".into()),
        base_amount: tokens as u64,
        quote_amount: sol,
        user_quote_amount: sol,
        // Post-trade reserves as the event records them.
        base_reserve: None,
        quote_reserve: None,
        virtual_base_reserve: Some((pre_vb - tokens) as u64),
        virtual_quote_reserve: Some((pre_vq + u128::from(sol)) as i128),
        ..base_swap()
    };
    assert_eq!(quote::predict_swap(&s).unwrap(), tokens);
}

/// pump.fun token-target `buy`: ceil quote for the requested base amount.
#[test]
fn pumpfun_token_target_buy_uses_ceil() {
    let pre_vq: u128 = 30_000_000_000;
    let pre_vb: u128 = 1_073_000_000_000_000;
    let target: u64 = 29_208_967_111_479;
    let cost = quote::predict_token_target_quote(&DecodedSwap {
        venue: Venue::PumpFun,
        is_buy: true,
        ix_name: Some("buy".into()),
        base_amount: target,
        quote_amount: 0,
        user_quote_amount: 0,
        base_reserve: None,
        quote_reserve: None,
        virtual_base_reserve: Some((pre_vb - u128::from(target)) as u64),
        // Post-trade quote reserve == pre (no quote delta modelled here).
        virtual_quote_reserve: Some(pre_vq as i128),
        ..base_swap()
    })
    .unwrap();
    assert_eq!(
        cost,
        (pre_vq * u128::from(target)).div_ceil(pre_vb - u128::from(target))
    );
}

#[test]
fn instruction_classification_is_strict() {
    let mut s = base_swap();
    s.venue = Venue::PumpSwap;
    s.is_buy = true;
    s.ix_name = Some("buy_exact_quote_in_v2".into());
    assert_eq!(quote::classify(&s).unwrap(), Instruction::BuyExactIn);
    s.ix_name = Some("something_else".into());
    assert_eq!(quote::classify(&s), Err(QuoteError::UnsupportedInstruction));
}

// --- Official SDK fee arithmetic (client-side) -----------------------------

#[test]
fn official_fee_is_ceil() {
    // ceil(987653 * 95 / 10000) = 9383 (matches the real TradeEvent).
    assert_eq!(quote::fee_ceil(987_653, 95), Some(9_383));
    assert_eq!(quote::fee_ceil(0, 100), Some(0));
    assert_eq!(quote::fee_ceil(1, 1), Some(1)); // ceil(0.0001)
}

#[test]
fn official_buy_input_uses_amount_minus_one_and_inversion() {
    // input = (amount-1)*10000/(totalBps+10000)
    assert_eq!(quote::buy_input_from_gross(1_000_000, 125), Some(987_653));
    assert_eq!(quote::buy_input_from_gross(0, 125), None);
    // No fee -> identity minus one.
    assert_eq!(quote::buy_input_from_gross(50_001, 0), Some(50_000));
}

#[test]
fn official_token_target_sol_cost_uses_plus_one() {
    // floor(x*vq/(vt-x)) + 1, not a generic ceiling.
    let vt = 1_000_000u128;
    let vq = 5_000u128;
    let x = 1_000u128;
    let floor = vq * x / (vt - x);
    assert_eq!(quote::token_target_sol_cost(vt, vq, x), Some(floor + 1));
}

#[test]
fn official_sell_net_subtracts_gated_creator_fee() {
    let gross = 987_653u128;
    // Protocol only (no creator set).
    assert_eq!(
        quote::sell_net_from_gross(gross, 95, None),
        Some(gross - 9_383)
    );
    // Protocol + creator.
    assert_eq!(
        quote::sell_net_from_gross(gross, 95, Some(30)),
        Some(gross - 9_383 - 2_963)
    );
}
