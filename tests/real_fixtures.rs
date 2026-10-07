//! Decoder validation against real mainnet bytes.
//!
//! The fixtures in `tests/fixtures/` are real captures (no credentials), so
//! these tests pin the decoder layouts to actual on-chain data while staying
//! deterministic and network-independent.

use neurone::decode::{self, Venue};
use neurone::events::MarketKey;

fn hex(s: &str) -> Vec<u8> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

fn key(s: &str) -> MarketKey {
    MarketKey::from_base58(s).expect("base58 pubkey")
}

#[test]
fn real_pumpfun_bonding_curve_decodes() {
    let data = hex(include_str!("../fixtures/pumpfun_bonding_curve.hex"));
    assert_eq!(data.len(), 125, "current bonding-curve layout is 125 bytes");
    let d = decode::decode_account(&key("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"), &data)
        .expect("decode real curve");
    assert_eq!(d.venue, Venue::PumpFun);
    assert_eq!(d.virtual_base_reserve, Some(1_072_999_996_530_634));
    assert_eq!(d.virtual_quote_reserve, Some(30_000_000_100));
    assert_eq!(d.base_reserve, Some(793_099_996_530_634));
    assert_eq!(d.quote_reserve, Some(100));
    assert_eq!(d.complete, Some(false));
}

#[test]
fn real_pumpfun_trade_event_decodes() {
    let data = hex(include_str!("../fixtures/pumpfun_trade_event.hex"));
    let s = decode::decode_event(&data).expect("decode real trade event");
    assert_eq!(s.venue, Venue::PumpFun);
    assert!(!s.is_buy, "fixture is a sell");
    assert_eq!(s.quote_amount, 987_653); // sol_amount
    assert_eq!(s.base_amount, 35_323_892_477); // token_amount
    assert_eq!(s.quote_reserve, Some(100));
    assert_eq!(s.base_reserve, Some(793_099_996_530_634));
    // Protocol fee (9,383) + creator fee (2,963) lamports.
    assert_eq!(s.fee_quote, 12_346);
    assert_eq!(
        s.base_mint.map(|m| m.to_base58()).as_deref(),
        Some("DdtKUh7bJKU7Dom2SMp1uHicr32o2VRR9hosjwRypump")
    );
}

#[test]
fn curve_account_and_trade_event_agree() {
    // Both fixtures describe the same market, so the reserves must match.
    let curve = decode::decode_account(
        &key("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"),
        &hex(include_str!("../fixtures/pumpfun_bonding_curve.hex")),
    )
    .unwrap();
    let trade =
        decode::decode_event(&hex(include_str!("../fixtures/pumpfun_trade_event.hex"))).unwrap();
    assert_eq!(curve.base_reserve, trade.base_reserve.map(u128::from));
    assert_eq!(curve.quote_reserve, trade.quote_reserve.map(u128::from));
}

/// The strongest decoder test: the bonding-curve PDA derived from the real
/// mint must equal the real on-chain curve address. This validates the PDA
/// algorithm against mainnet data, not a self-consistent assumption.
#[test]
fn bonding_curve_pda_matches_real_mainnet_address() {
    let mint = key("DdtKUh7bJKU7Dom2SMp1uHicr32o2VRR9hosjwRypump");
    let expected = key("112heubZsHqSNJpc3QHTz6FjQDTLQpXiSDGyjALEZGr");
    let derived = decode::pumpfun::bonding_curve(&mint).expect("pda");
    assert_eq!(
        derived, expected,
        "PDA(mint) must equal the real curve account"
    );
    assert_eq!(derived.to_base58(), expected.to_base58());
}

#[test]
fn real_pumpswap_pool_decodes() {
    let data = hex(include_str!("../fixtures/pumpswap_pool.hex"));
    let d = decode::decode_account(&key("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"), &data)
        .expect("decode real pool");
    assert_eq!(d.venue, Venue::PumpSwap);
    assert!(d.base_mint.is_some());
    assert!(d.quote_mint.is_some());
    assert!(d.pool_base_token_account.is_some());
    assert!(d.pool_quote_token_account.is_some());
}

#[test]
fn real_pumpswap_swap_events_decode() {
    let buy =
        decode::decode_event(&hex(include_str!("../fixtures/pumpswap_buy_event.hex"))).unwrap();
    assert_eq!(buy.venue, Venue::PumpSwap);
    assert!(buy.is_buy);
    assert!(buy.base_amount > 0);
    assert!(buy.quote_amount > 0);
    assert!(buy.base_reserve.is_some() && buy.quote_reserve.is_some());

    let sell =
        decode::decode_event(&hex(include_str!("../fixtures/pumpswap_sell_event.hex"))).unwrap();
    assert_eq!(sell.venue, Venue::PumpSwap);
    assert!(!sell.is_buy);
    assert!(sell.base_reserve.is_some() && sell.quote_reserve.is_some());
}

#[test]
fn malformed_and_wrong_program_inputs_are_rejected() {
    let curve = hex(include_str!("../fixtures/pumpfun_bonding_curve.hex"));
    // Wrong program id for the data.
    assert!(
        decode::decode_account(&key("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA"), &curve)
            .is_none()
    );
    // Truncated.
    assert!(decode::decode_account(
        &key("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"),
        &curve[..20]
    )
    .is_none());
    // Bit-flipped discriminator.
    let mut bad = curve.clone();
    bad[0] ^= 0xFF;
    assert!(
        decode::decode_account(&key("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"), &bad).is_none()
    );
}
