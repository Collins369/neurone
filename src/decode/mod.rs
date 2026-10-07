//! Protocol decoding.
//!
//! Converts raw Yellowstone payloads into protocol-derived market primitives.
//! Everything here is a *pure function* over bytes so it is deterministic and
//! testable against fixtures without a network.
//!
//! Venues are decoded against the **official pump.fun IDLs**
//! (`pump-fun/pump-public-docs`):
//! * `pump`     — the bonding-curve program (`BondingCurve` account, `TradeEvent`);
//! * `pump_amm` — the pump.swap AMM (`Pool` account, `BuyEvent`/`SellEvent`).
//!
//! Layouts are positional Borsh; a decoder reads a validated prefix and stops,
//! so trailing fields added by a newer program version are ignored rather than
//! misinterpreted.

pub mod borsh;
pub mod pda;
pub mod pump_amm;
pub mod pumpfun;

use crate::events::MarketKey;

/// Which on-chain venue a market belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Venue {
    /// pump.fun bonding curve (`pump` program).
    PumpFun,
    /// pump.swap constant-product AMM (`pump_amm` program).
    PumpSwap,
}

impl Venue {
    pub fn as_str(self) -> &'static str {
        match self {
            Venue::PumpFun => "pumpfun",
            Venue::PumpSwap => "pumpswap",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Venue::PumpFun => 0,
            Venue::PumpSwap => 1,
        }
    }
}

/// Protocol-derived state read from a tracked account.
///
/// All reserves are **raw on-chain integer units**: base reserves in raw base
/// token units, quote reserves in raw quote units (lamports when the quote mint
/// is wrapped SOL). No decimal scaling is applied here — decimals live on the
/// mint accounts, which this milestone does not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedAccount {
    pub venue: Venue,
    pub base_mint: Option<MarketKey>,
    pub quote_mint: Option<MarketKey>,
    pub base_reserve: Option<u128>,
    pub quote_reserve: Option<u128>,
    pub virtual_base_reserve: Option<u128>,
    pub virtual_quote_reserve: Option<i128>,
    pub token_total_supply: Option<u128>,
    /// Bonding-curve completion flag (pump.fun only).
    pub complete: Option<bool>,
    pub creator: Option<MarketKey>,
    /// pump.swap pool token accounts (needed later for executable balance reads).
    pub pool_base_token_account: Option<MarketKey>,
    pub pool_quote_token_account: Option<MarketKey>,
}

/// A decoded swap/trade observed in a transaction.
///
/// `market_key` is the account that owns the market state (the bonding-curve
/// PDA for pump.fun, the pool account for pump.swap). `base_amount`/`quote_amount`
/// are the executed amounts in raw units (gross quote paid/received).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedSwap {
    pub venue: Venue,
    pub market_key: MarketKey,
    pub base_mint: Option<MarketKey>,
    pub quote_mint: Option<MarketKey>,
    pub is_buy: bool,
    pub base_amount: u64,
    pub quote_amount: u64,
    pub base_reserve: Option<u64>,
    pub quote_reserve: Option<u64>,
    pub virtual_quote_reserve: Option<i128>,
    /// Fees in raw quote units (protocol + LP + creator where known).
    pub fee_quote: u64,
    /// Total fee in basis points, when the event exposes it.
    pub fee_bps: Option<u64>,
    pub timestamp: Option<i64>,
    /// Instruction/log name, when the event carries one (pump.swap).
    pub ix_name: Option<String>,
}

/// A newly created market observed in a `CreateEvent`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreatedMarket {
    pub venue: Venue,
    pub market_key: MarketKey,
    pub mint: MarketKey,
    pub quote_mint: Option<MarketKey>,
    pub virtual_base_reserve: Option<u128>,
    pub virtual_quote_reserve: Option<i128>,
    pub base_reserve: Option<u128>,
    pub token_total_supply: Option<u128>,
    pub timestamp: Option<i64>,
}

/// Decode a tracked account's data based on its owning program.
pub fn decode_account(owner: &MarketKey, data: &[u8]) -> Option<DecodedAccount> {
    match owner.as_bytes() {
        id if id == &pumpfun::PROGRAM_ID => pumpfun::decode_account(data),
        id if id == &pump_amm::PROGRAM_ID => pump_amm::decode_account(data),
        _ => None,
    }
}

/// Decode an Anchor event payload ("Program data: ...") by discriminator.
///
/// Discriminators are 8-byte hashes and effectively unique, so dispatch by
/// discriminator alone is safe; callers additionally only look at payloads
/// emitted inside transactions that touched a known program.
pub fn decode_event(payload: &[u8]) -> Option<DecodedSwap> {
    if payload.len() < 8 {
        return None;
    }
    if payload[..8] == pumpfun::TRADE_EVENT_DISC {
        return pumpfun::decode_trade_event(payload);
    }
    if payload[..8] == pump_amm::BUY_EVENT_DISC {
        return pump_amm::decode_swap_event(payload, true);
    }
    if payload[..8] == pump_amm::SELL_EVENT_DISC {
        return pump_amm::decode_swap_event(payload, false);
    }
    None
}

/// Whether a payload's discriminator matches a protocol event we understand.
pub fn is_known_event_discriminator(payload: &[u8]) -> bool {
    payload.len() >= 8
        && (payload[..8] == pumpfun::TRADE_EVENT_DISC
            || payload[..8] == pumpfun::CREATE_EVENT_DISC
            || payload[..8] == pump_amm::BUY_EVENT_DISC
            || payload[..8] == pump_amm::SELL_EVENT_DISC)
}

/// Whether an account owner is a venue program whose account data we decode.
pub fn is_venue_program(owner: &MarketKey) -> bool {
    owner.as_bytes() == &pumpfun::PROGRAM_ID || owner.as_bytes() == &pump_amm::PROGRAM_ID
}

/// Decode a pump.fun `CreateEvent`.
pub fn decode_create_event(payload: &[u8]) -> Option<CreatedMarket> {
    if payload.len() < 8 || payload[..8] != pumpfun::CREATE_EVENT_DISC {
        return None;
    }
    pumpfun::decode_create_event(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_payload_is_rejected() {
        assert!(decode_event(&[]).is_none());
        assert!(decode_event(&[0u8; 8]).is_none());
    }

    #[test]
    fn program_ids_are_distinct() {
        assert_ne!(pumpfun::PROGRAM_ID, pump_amm::PROGRAM_ID);
    }
}
