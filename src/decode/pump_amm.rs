//! pump.swap AMM (`pump_amm`) decoding — the second venue.
//!
//! Source of truth: `pump-fun/pump-public-docs` → `idl/pump_amm.json`
//! (program `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA`).
//!
//! * `Pool` account — discriminator `[241,154,109,4,17,177,109,188]`.
//! * `BuyEvent`  — discriminator `[103,244,82,31,44,245,119,119]`.
//! * `SellEvent` — discriminator `[62,47,55,10,165,3,220,42]`.
//!
//! Selection rationale: pump.swap is where pump.fun tokens migrate after curve
//! completion, so it is the highest-relevance second venue for meme markets,
//! its IDL is official, and the swap events expose executable reserves
//! (`pool_base_token_reserves` / `pool_quote_token_reserves`) directly.
//!
//! Unlike the bonding curve, the `Pool` account does not store current
//! base/quote reserves (they live in the pool token accounts), so pool-level
//! reserves come from the swap events.

use crate::decode::borsh::Reader;
use crate::decode::{DecodedAccount, DecodedSwap, Venue};

/// `pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA`
pub const PROGRAM_ID: [u8; 32] = [
    12, 20, 222, 252, 130, 94, 198, 118, 148, 37, 8, 24, 187, 101, 64, 101, 244, 41, 141, 49, 86,
    213, 113, 180, 212, 248, 9, 12, 24, 233, 168, 99,
];

pub const POOL_DISC: [u8; 8] = [241, 154, 109, 4, 17, 177, 109, 188];
pub const BUY_EVENT_DISC: [u8; 8] = [103, 244, 82, 31, 44, 245, 119, 119];
pub const SELL_EVENT_DISC: [u8; 8] = [62, 47, 55, 10, 165, 3, 220, 42];

/// Decode a `Pool` account (mints, token accounts, virtual quote reserves).
pub fn decode_account(data: &[u8]) -> Option<DecodedAccount> {
    let mut r = Reader::new(data);
    if !r.starts_with(&POOL_DISC) {
        return None;
    }
    r.take(8)?;
    let _pool_bump = r.u8()?;
    let _index = r.u16()?;
    let creator = r.pubkey()?;
    let base_mint = r.pubkey()?;
    let quote_mint = r.pubkey()?;
    let _lp_mint = r.pubkey()?;
    let pool_base_token_account = r.pubkey()?;
    let pool_quote_token_account = r.pubkey()?;
    let _lp_supply = r.u64()?;
    let _coin_creator = r.pubkey()?;
    let _is_mayhem_mode = r.bool();
    let _is_cashback_coin = r.bool();
    let virtual_quote_reserve = r.i128();

    Some(DecodedAccount {
        venue: Venue::PumpSwap,
        base_mint: Some(base_mint),
        quote_mint: Some(quote_mint),
        // Current reserves are not stored in the Pool account.
        base_reserve: None,
        quote_reserve: None,
        virtual_base_reserve: None,
        virtual_quote_reserve,
        token_total_supply: None,
        complete: None,
        creator: Some(creator),
        pool_base_token_account: Some(pool_base_token_account),
        pool_quote_token_account: Some(pool_quote_token_account),
    })
}

/// Decode a `BuyEvent` (`is_buy = true`) or `SellEvent` (`is_buy = false`).
///
/// Both events share an identical leading field sequence through
/// `coin_creator_fee`; only later fields diverge, so we read the shared prefix
/// and stop. `market_key` is the pool account.
pub fn decode_swap_event(payload: &[u8], is_buy: bool) -> Option<DecodedSwap> {
    let expected = if is_buy {
        BUY_EVENT_DISC
    } else {
        SELL_EVENT_DISC
    };
    let mut r = Reader::new(payload);
    if !r.starts_with(&expected) {
        return None;
    }
    r.take(8)?;
    let timestamp = r.i64()?;
    let base_amount = r.u64()?; // base_amount_out (buy) / base_amount_in (sell)
    let _limit = r.u64()?; // max_quote_amount_in / min_quote_amount_out
    let _user_base_reserves = r.u64()?;
    let _user_quote_reserves = r.u64()?;
    let pool_base_reserves = r.u64()?;
    let pool_quote_reserves = r.u64()?;
    let quote_amount = r.u64()?; // quote_amount_in (buy) / quote_amount_out (sell)
    let lp_fee_bps = r.u64()?;
    let lp_fee = r.u64()?;
    let protocol_fee_bps = r.u64()?;
    let protocol_fee = r.u64()?;
    let _quote_with_fee = r.u64()?;
    let user_quote_amount = r.u64()?; // user_quote_amount_in (buy) / _out (sell)
    let pool = r.pubkey()?;
    let _user = r.pubkey()?;
    let _user_base_token_account = r.pubkey()?;
    let _user_quote_token_account = r.pubkey()?;
    let _protocol_fee_recipient = r.pubkey()?;
    let _protocol_fee_recipient_token_account = r.pubkey()?;
    let _coin_creator = r.pubkey()?;
    let _coin_creator_fee_bps = r.u64()?;
    let coin_creator_fee = r.u64().unwrap_or(0);

    // Appended tail. `buy` carries ix_name before the appended virtual reserve;
    // `sell` has no ix_name. Both end with a signed i128 virtual_quote_reserves
    // followed by `can_boost, base_supply, holder_rewards_bps, holder_rewards`.
    let mut ix_name = None;
    if is_buy {
        // track_volume(1) + 5 u64, then the ix_name string.
        if r.take(1 + 8 * 5).is_some() {
            ix_name = r.string();
        }
    }
    // 4 u64 (cashback/buyback pairs), then the i128 virtual reserve.
    let virtual_quote_reserve = if r.take(8 * 4).is_some() {
        r.i128()
    } else {
        None
    };

    Some(DecodedSwap {
        venue: Venue::PumpSwap,
        market_key: pool,
        base_mint: None,
        quote_mint: None,
        is_buy,
        base_amount,
        quote_amount,
        user_quote_amount,
        base_reserve: Some(pool_base_reserves),
        quote_reserve: Some(pool_quote_reserves),
        virtual_base_reserve: None,
        virtual_quote_reserve,
        fee_quote: lp_fee
            .saturating_add(protocol_fee)
            .saturating_add(coin_creator_fee),
        fee_bps: Some(lp_fee_bps.saturating_add(protocol_fee_bps)),
        timestamp: Some(timestamp),
        ix_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::MarketKey;

    fn push_pubkey(buf: &mut Vec<u8>, b: u8) {
        buf.extend_from_slice(&[b; 32]);
    }

    #[test]
    fn decodes_pool_account() {
        let mut data = Vec::new();
        data.extend_from_slice(&POOL_DISC);
        data.push(255); // pool_bump
        data.extend_from_slice(&1u16.to_le_bytes()); // index
        push_pubkey(&mut data, 1); // creator
        push_pubkey(&mut data, 2); // base_mint
        push_pubkey(&mut data, 3); // quote_mint
        push_pubkey(&mut data, 4); // lp_mint
        push_pubkey(&mut data, 5); // pool_base_token_account
        push_pubkey(&mut data, 6); // pool_quote_token_account
        data.extend_from_slice(&1_000u64.to_le_bytes()); // lp_supply
        push_pubkey(&mut data, 7); // coin_creator
        data.push(0); // is_mayhem
        data.push(0); // is_cashback
        data.extend_from_slice(&(-5i128).to_le_bytes()); // virtual_quote_reserves

        let d = decode_account(&data).expect("decode pool");
        assert_eq!(d.venue, Venue::PumpSwap);
        assert_eq!(d.base_mint, Some(MarketKey([2u8; 32])));
        assert_eq!(d.quote_mint, Some(MarketKey([3u8; 32])));
        assert_eq!(d.pool_base_token_account, Some(MarketKey([5u8; 32])));
        assert_eq!(d.pool_quote_token_account, Some(MarketKey([6u8; 32])));
        assert_eq!(d.virtual_quote_reserve, Some(-5));
    }

    #[test]
    fn decodes_buy_event_prefix() {
        let pool = MarketKey([42u8; 32]);
        let mut p = Vec::new();
        p.extend_from_slice(&BUY_EVENT_DISC);
        p.extend_from_slice(&1_700_000_000i64.to_le_bytes()); // timestamp
        p.extend_from_slice(&500u64.to_le_bytes()); // base_amount_out
        p.extend_from_slice(&2_000u64.to_le_bytes()); // max_quote_amount_in
        p.extend_from_slice(&100u64.to_le_bytes()); // user_base_reserves
        p.extend_from_slice(&200u64.to_le_bytes()); // user_quote_reserves
        p.extend_from_slice(&1_000_000u64.to_le_bytes()); // pool_base_reserves
        p.extend_from_slice(&5_000_000u64.to_le_bytes()); // pool_quote_reserves
        p.extend_from_slice(&1_500u64.to_le_bytes()); // quote_amount_in
        p.extend_from_slice(&25u64.to_le_bytes()); // lp_fee_bps
        p.extend_from_slice(&3u64.to_le_bytes()); // lp_fee
        p.extend_from_slice(&5u64.to_le_bytes()); // protocol_fee_bps
        p.extend_from_slice(&7u64.to_le_bytes()); // protocol_fee
        p.extend_from_slice(&1_510u64.to_le_bytes()); // quote_amount_in_with_lp_fee
        p.extend_from_slice(&1_520u64.to_le_bytes()); // user_quote_amount_in
        p.extend_from_slice(pool.as_bytes());
        push_pubkey(&mut p, 10); // user
        push_pubkey(&mut p, 11); // user_base_token_account
        push_pubkey(&mut p, 12); // user_quote_token_account
        push_pubkey(&mut p, 13); // protocol_fee_recipient
        push_pubkey(&mut p, 14); // protocol_fee_recipient_token_account
        push_pubkey(&mut p, 15); // coin_creator
        p.extend_from_slice(&10u64.to_le_bytes()); // coin_creator_fee_bps
        p.extend_from_slice(&2u64.to_le_bytes()); // coin_creator_fee

        let s = decode_swap_event(&p, true).expect("decode buy");
        assert_eq!(s.venue, Venue::PumpSwap);
        assert_eq!(s.market_key, pool);
        assert!(s.is_buy);
        assert_eq!(s.base_amount, 500);
        assert_eq!(s.quote_amount, 1_500);
        assert_eq!(s.base_reserve, Some(1_000_000));
        assert_eq!(s.quote_reserve, Some(5_000_000));
        assert_eq!(s.fee_quote, 3 + 7 + 2);
    }

    #[test]
    fn rejects_wrong_discriminator() {
        assert!(decode_swap_event(&BUY_EVENT_DISC, false).is_none());
        assert!(decode_account(&[0u8; 128]).is_none());
    }
}
