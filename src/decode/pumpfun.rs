//! pump.fun bonding-curve program decoding.
//!
//! Source of truth: `pump-fun/pump-public-docs` → `idl/pump.json`
//! (program `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P`).
//!
//! * `BondingCurve` account — discriminator `[23,183,248,55,96,216,172,96]`.
//! * `TradeEvent` — discriminator `[189,219,127,211,78,230,97,238]`.
//! * `CreateEvent` — discriminator `[27,114,169,77,222,235,99,118]`.
//!
//! A bonding curve is a PDA of the mint: `find_program_address(["bonding-curve",
//! mint], pump_program)`.

use crate::decode::borsh::Reader;
use crate::decode::pda;
use crate::decode::{CreatedMarket, DecodedAccount, DecodedSwap, Venue};
use crate::events::MarketKey;

/// `6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P`
pub const PROGRAM_ID: [u8; 32] = [
    1, 86, 224, 246, 147, 102, 90, 207, 68, 219, 21, 104, 191, 23, 91, 170, 81, 137, 203, 151, 245,
    210, 255, 59, 101, 93, 43, 182, 253, 109, 24, 176,
];

pub const BONDING_CURVE_DISC: [u8; 8] = [23, 183, 248, 55, 96, 216, 172, 96];
pub const TRADE_EVENT_DISC: [u8; 8] = [189, 219, 127, 211, 78, 230, 97, 238];
pub const CREATE_EVENT_DISC: [u8; 8] = [27, 114, 169, 77, 222, 235, 99, 118];
pub const COMPLETE_EVENT_DISC: [u8; 8] = [95, 114, 97, 156, 212, 46, 152, 8];

const BONDING_CURVE_SEED: &[u8] = b"bonding-curve";

/// Compute the bonding-curve PDA for a mint.
pub fn bonding_curve(mint: &MarketKey) -> Option<MarketKey> {
    pda::find_program_address(&[BONDING_CURVE_SEED, mint.as_bytes()], &PROGRAM_ID)
        .map(|(addr, _)| MarketKey(addr))
}

/// Decode a `BondingCurve` account.
///
/// The layout has grown over time; only the leading fields are required, and
/// the trailing ones (creator, quote mint, fee config…) are read when present.
pub fn decode_account(data: &[u8]) -> Option<DecodedAccount> {
    let mut r = Reader::new(data);
    if !r.starts_with(&BONDING_CURVE_DISC) {
        return None;
    }
    r.take(8)?; // discriminator
    let virtual_base = r.u64()? as u128;
    let virtual_quote = r.u64()? as u128;
    let real_base = r.u64()? as u128;
    let real_quote = r.u64()? as u128;
    let token_total_supply = r.u64()? as u128;
    let complete = r.bool()?;

    // Optional trailing fields (present on current accounts).
    let creator = r.pubkey();
    let _is_mayhem = r.bool();
    let _is_cashback = r.bool();
    let quote_mint = r.pubkey();

    Some(DecodedAccount {
        venue: Venue::PumpFun,
        base_mint: None,
        quote_mint,
        base_reserve: Some(real_base),
        quote_reserve: Some(real_quote),
        virtual_base_reserve: Some(virtual_base),
        virtual_quote_reserve: Some(virtual_quote as i128),
        token_total_supply: Some(token_total_supply),
        complete: Some(complete),
        creator,
        pool_base_token_account: None,
        pool_quote_token_account: None,
    })
}

/// Decode a `TradeEvent`.
///
/// The fixed prefix through `real_token_reserves` is required; the fee fields
/// that follow are read when present. Fields added even later (shareholders,
/// quote mint, mayhem/cashback config) are intentionally ignored.
pub fn decode_trade_event(payload: &[u8]) -> Option<DecodedSwap> {
    let mut r = Reader::new(payload);
    if !r.starts_with(&TRADE_EVENT_DISC) {
        return None;
    }
    r.take(8)?;
    let mint = r.pubkey()?;
    let sol_amount = r.u64()?;
    let token_amount = r.u64()?;
    let is_buy = r.bool()?;
    let _user = r.pubkey()?;
    let timestamp = r.i64()?;
    let virtual_sol_reserves = r.u64()?;
    let virtual_token_reserves = r.u64()?;
    let real_sol_reserves = r.u64()?;
    let real_token_reserves = r.u64()?;

    // Optional fee tail, then track_volume + 4 u64 counters, then `ix_name`.
    let mut fee_quote = 0u64;
    let mut creator_fee_quote = 0u64;
    let mut fee_bps = None;
    if r.pubkey().is_some() {
        fee_bps = r.u64();
        if let Some(fee) = r.u64() {
            fee_quote = fee;
        }
        if r.pubkey().is_some() {
            let _creator_bps = r.u64();
            if let Some(cf) = r.u64() {
                creator_fee_quote = cf;
            }
        }
    }
    // Appended tail: track_volume + 4 counters, ix_name, mayhem + cashback/
    // buyback, shareholders vec, then the quote-aware fields. Non-SOL (e.g.
    // USDC) pairs carry `sol_amount == 0` and put the value in `quote_amount`
    // with `virtual_quote_reserves` / `real_quote_reserves`.
    let mut ix_name = None;
    let mut quote_amount = None;
    let mut virtual_quote_reserves = None;
    let mut real_quote_reserves = None;
    let mut _quote_mint = None;
    if r.take(1 + 8 * 4).is_some() {
        ix_name = r.string();
        let _mayhem = r.bool();
        let _cashback_bps = r.u64();
        let _cashback = r.u64();
        let _buyback_bps = r.u64();
        let _buyback = r.u64();
        if let Some(n) = r.u32() {
            r.take(n as usize * (32 + 2));
        }
        _quote_mint = r.pubkey();
        quote_amount = r.u64();
        virtual_quote_reserves = r.u64();
        real_quote_reserves = r.u64();
    }

    // Choose the quote side: SOL-paired coins use the leading sol fields;
    // non-SOL pairs (sol_amount == 0) use the appended quote fields.
    let (eff_quote_amount, eff_virtual_quote, eff_real_quote) =
        if sol_amount == 0 && quote_amount.unwrap_or(0) > 0 {
            (
                quote_amount.unwrap_or(0),
                virtual_quote_reserves.unwrap_or(0) as i128,
                real_quote_reserves.unwrap_or(0),
            )
        } else {
            (sol_amount, virtual_sol_reserves as i128, real_sol_reserves)
        };

    Some(DecodedSwap {
        venue: Venue::PumpFun,
        market_key: bonding_curve(&mint)?,
        base_mint: Some(mint),
        quote_mint: None,
        is_buy,
        base_amount: token_amount,
        quote_amount: eff_quote_amount,
        user_quote_amount: eff_quote_amount,
        base_reserve: Some(real_token_reserves),
        quote_reserve: Some(eff_real_quote),
        virtual_base_reserve: Some(virtual_token_reserves),
        virtual_quote_reserve: Some(eff_virtual_quote),
        fee_quote: fee_quote.saturating_add(creator_fee_quote),
        fee_bps,
        timestamp: Some(timestamp),
        ix_name,
    })
}

/// Decode a `CreateEvent`.
///
/// Field order: name, symbol, uri, mint, bonding_curve, user, creator,
/// timestamp, virtual_token_reserves, virtual_sol_reserves, real_token_reserves,
/// token_total_supply, token_program, is_mayhem_mode, is_cashback_enabled,
/// quote_mint, virtual_quote_reserves, ...
pub fn decode_create_event(payload: &[u8]) -> Option<CreatedMarket> {
    let mut r = Reader::new(payload);
    if !r.starts_with(&CREATE_EVENT_DISC) {
        return None;
    }
    r.take(8)?;
    r.string()?; // name
    r.string()?; // symbol
    r.string()?; // uri
    let mint = r.pubkey()?;
    let market_key = r.pubkey()?;
    let _user = r.pubkey()?;
    let _creator = r.pubkey()?;
    let timestamp = r.i64()?;
    let virtual_base = r.u64().map(u128::from);
    let _virtual_quote_sol = r.u64();
    let real_base = r.u64().map(u128::from);
    let token_total_supply = r.u64().map(u128::from);
    let _token_program = r.pubkey();
    let _is_mayhem = r.bool();
    let _is_cashback = r.bool();
    let quote_mint = r.pubkey();
    let virtual_quote = r.u64().map(|v| v as i128);
    Some(CreatedMarket {
        venue: Venue::PumpFun,
        market_key,
        mint,
        quote_mint,
        virtual_base_reserve: virtual_base,
        virtual_quote_reserve: virtual_quote,
        base_reserve: real_base,
        token_total_supply,
        timestamp: Some(timestamp),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_pubkey(buf: &mut Vec<u8>, b: u8) {
        buf.extend_from_slice(&[b; 32]);
    }

    #[test]
    fn bonding_curve_pda_is_deterministic() {
        let mint = MarketKey([9u8; 32]);
        let a = bonding_curve(&mint).unwrap();
        let b = bonding_curve(&mint).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, mint);
    }

    #[test]
    fn decodes_current_bonding_curve_layout() {
        let mut data = Vec::new();
        data.extend_from_slice(&BONDING_CURVE_DISC);
        data.extend_from_slice(&1_000_000_000u64.to_le_bytes()); // virtual base
        data.extend_from_slice(&30_000_000_000u64.to_le_bytes()); // virtual quote
        data.extend_from_slice(&800_000_000u64.to_le_bytes()); // real base
        data.extend_from_slice(&5_000_000_000u64.to_le_bytes()); // real quote
        data.extend_from_slice(&1_000_000_000u64.to_le_bytes()); // supply
        data.push(0); // complete
        push_pubkey(&mut data, 7); // creator
        data.push(0); // is_mayhem_mode
        data.push(0); // is_cashback_coin
        push_pubkey(&mut data, 8); // quote_mint
        data.extend_from_slice(&100u64.to_le_bytes()); // creator_fee_bps
        data.push(1); // can_edit_creator_fee
        data.push(0); // is_holder_reward

        let d = decode_account(&data).expect("decode");
        assert_eq!(d.venue, Venue::PumpFun);
        assert_eq!(d.base_reserve, Some(800_000_000));
        assert_eq!(d.quote_reserve, Some(5_000_000_000));
        assert_eq!(d.virtual_quote_reserve, Some(30_000_000_000));
        assert_eq!(d.complete, Some(false));
        assert_eq!(d.creator, Some(MarketKey([7u8; 32])));
        assert_eq!(d.quote_mint, Some(MarketKey([8u8; 32])));
    }

    #[test]
    fn decodes_legacy_short_bonding_curve() {
        // 49-byte legacy layout: prefix only.
        let mut data = Vec::new();
        data.extend_from_slice(&BONDING_CURVE_DISC);
        data.extend_from_slice(&1u64.to_le_bytes());
        data.extend_from_slice(&2u64.to_le_bytes());
        data.extend_from_slice(&3u64.to_le_bytes());
        data.extend_from_slice(&4u64.to_le_bytes());
        data.extend_from_slice(&5u64.to_le_bytes());
        data.push(0);
        assert_eq!(data.len(), 49);
        let d = decode_account(&data).expect("decode");
        assert_eq!(d.creator, None);
        assert_eq!(d.quote_mint, None);
    }

    #[test]
    fn rejects_wrong_discriminator_and_truncation() {
        assert!(decode_account(&[0u8; 64]).is_none());
        let mut short = BONDING_CURVE_DISC.to_vec();
        short.extend_from_slice(&[0u8; 8]);
        assert!(decode_account(&short).is_none());
    }

    #[test]
    fn decodes_trade_event_prefix() {
        let mint = MarketKey([5u8; 32]);
        let mut p = Vec::new();
        p.extend_from_slice(&TRADE_EVENT_DISC);
        p.extend_from_slice(mint.as_bytes());
        p.extend_from_slice(&1_000_000u64.to_le_bytes()); // sol_amount
        p.extend_from_slice(&2_000_000u64.to_le_bytes()); // token_amount
        p.push(1); // is_buy
        push_pubkey(&mut p, 3); // user
        p.extend_from_slice(&1_700_000_000i64.to_le_bytes()); // timestamp
        p.extend_from_slice(&30_000_000_000u64.to_le_bytes()); // virtual sol
        p.extend_from_slice(&1_000_000_000u64.to_le_bytes()); // virtual token
        p.extend_from_slice(&5_000_000_000u64.to_le_bytes()); // real sol
        p.extend_from_slice(&800_000_000u64.to_le_bytes()); // real token

        let s = decode_trade_event(&p).expect("decode");
        assert!(s.is_buy);
        assert_eq!(s.base_amount, 2_000_000);
        assert_eq!(s.quote_amount, 1_000_000);
        assert_eq!(s.quote_reserve, Some(5_000_000_000));
        assert_eq!(s.base_mint, Some(mint));
        assert_eq!(s.market_key, bonding_curve(&mint).unwrap());
        assert_eq!(s.venue, Venue::PumpFun);
    }

    #[test]
    fn truncated_trade_event_is_rejected() {
        let mut p = TRADE_EVENT_DISC.to_vec();
        p.extend_from_slice(&[0u8; 10]);
        assert!(decode_trade_event(&p).is_none());
    }
}
