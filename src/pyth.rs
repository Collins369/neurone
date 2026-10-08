//! Pyth `PriceUpdateV2` decoding for the on-chain SOL/USD price feed.
//!
//! Pyth's Solana push feeds are normal Solana accounts owned by the Pyth
//! **receiver** program (`rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ`). The
//! sponsored SOL/USD feed account is
//! `7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE` (feed id
//! `ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d`), so it
//! can be streamed through the existing Yellowstone transport with a single
//! bounded account-address filter — no HTTP, no RPC polling, no extra transport.
//!
//! `PriceUpdateV2` layout (Borsh; Anchor discriminator first):
//!
//! ```text
//! @0   discriminator            8   = 22 f1 23 63 9d 7e f4 cd
//! @8   write_authority         32
//! @40  verification_level       1..2 (0 = Partial{num_signatures:u8}, 1 = Full)
//! @41+ feed_id                 32
//!      price                   i64
//!      conf                    u64
//!      exponent                i32
//!      publish_time            i64
//!      prev_publish_time       i64
//!      ema_price               i64
//!      ema_conf                u64
//!      posted_slot             u64
//! ```
//!
//! This module is deterministic and allocation-free; it performs no I/O.

/// Pyth receiver program (Solana mainnet).
pub const RECEIVER_PROGRAM_ID: [u8; 32] = [
    12, 20, 222, 252, 130, 94, 198, 118, 148, 37, 8, 24, 187, 101, 64, 101, 244, 41, 141, 49, 86,
    213, 113, 180, 212, 248, 9, 12, 24, 233, 168, 99,
];

/// `PriceUpdateV2` account discriminator.
pub const PRICE_UPDATE_V2_DISC: [u8; 8] = [0x22, 0xf1, 0x23, 0x63, 0x9d, 0x7e, 0xf4, 0xcd];

/// Pyth SOL/USD feed id (`ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d`).
pub const SOL_USD_FEED_ID: [u8; 32] = [
    0xef, 0x0d, 0x8b, 0x6f, 0xda, 0x2c, 0xeb, 0xa4, 0x1d, 0xa1, 0x5d, 0x40, 0x95, 0xd1, 0xda, 0x39,
    0x2a, 0x0d, 0x2f, 0x8e, 0xd0, 0xc6, 0xc7, 0xbc, 0x0f, 0x4c, 0xfa, 0xc8, 0xc2, 0x80, 0xb5, 0x6d,
];

/// Sponsored SOL/USD `PriceUpdateV2` account (Solana mainnet, shard 0).
pub const SOL_USD_PRICE_ACCOUNT: &str = "7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE";

/// `[u8; 32]` form of [`SOL_USD_PRICE_ACCOUNT`] for allocation-free matching.
pub const SOL_USD_PRICE_ACCOUNT_BYTES: [u8; 32] = [
    96, 49, 71, 4, 52, 13, 237, 223, 55, 31, 212, 36, 114, 20, 143, 36, 142, 157, 26, 109, 26, 94,
    178, 172, 58, 205, 139, 127, 213, 214, 178, 67,
];

/// A decoded SOL/USD Pyth price update.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SolUsdPriceUpdate {
    /// Raw price mantissa (Pyth `price`).
    pub price: i64,
    /// Raw confidence mantissa (Pyth `conf`).
    pub conf: u64,
    /// Base-10 exponent: `USD = price * 10^exponent`.
    pub exponent: i32,
    /// Pyth publish time (unix seconds).
    pub publish_time: i64,
    /// Previous publish time (unix seconds).
    pub prev_publish_time: i64,
    pub ema_price: i64,
    pub ema_conf: u64,
    /// Solana slot at which this update was posted.
    pub posted_slot: u64,
    /// Whether the update carried a `Full` verification level.
    pub full_verification: bool,
}

impl SolUsdPriceUpdate {
    /// USD price in **micros** (1e-6 USD), integer-only.
    ///
    /// `micros = price * 10^(exponent + 6)`. When `exponent < -6` the result is
    /// floored (sub-micro precision is discarded). Returns `None` for a
    /// non-positive price, a non-`Full` verification level, or any overflow.
    pub fn price_micros(&self) -> Option<u64> {
        if !self.full_verification || self.price <= 0 {
            return None;
        }
        scale_micros(self.price as u128, self.exponent)
    }

    /// Confidence interval half-width in micros (best-effort; `None` on overflow).
    pub fn conf_micros(&self) -> Option<u64> {
        scale_micros(u128::from(self.conf), self.exponent)
    }
}

/// Scale `mantissa * 10^exponent` into micro-units (`10^-6`).
fn scale_micros(mantissa: u128, exponent: i32) -> Option<u64> {
    let shift = exponent.checked_add(6)?; // micros = mantissa * 10^(exp+6)
    if shift >= 0 {
        let factor = 10u128.checked_pow(shift as u32)?;
        u64::try_from(mantissa.checked_mul(factor)?).ok()
    } else {
        let divisor = 10u128.checked_pow((-shift) as u32)?;
        u64::try_from(mantissa.checked_div(divisor)?).ok()
    }
}

/// Decode a `PriceUpdateV2` account and require it to be the SOL/USD feed.
pub fn decode_sol_usd_price_update(data: &[u8]) -> Option<SolUsdPriceUpdate> {
    if data.len() < 8 || data[..8] != PRICE_UPDATE_V2_DISC {
        return None;
    }
    // verification_level: 0 = Partial{num_signatures:u8} (2 bytes), 1 = Full (1).
    let level = *data.get(40)?;
    let (full_verification, mut o) = match level {
        0 => (false, 41 + 1),
        1 => (true, 41),
        _ => return None,
    };
    let feed_id = data.get(o..o + 32)?;
    if feed_id != SOL_USD_FEED_ID {
        return None;
    }
    o += 32;
    let price = i64::from_le_bytes(data.get(o..o + 8)?.try_into().ok()?);
    o += 8;
    let conf = u64::from_le_bytes(data.get(o..o + 8)?.try_into().ok()?);
    o += 8;
    let exponent = i32::from_le_bytes(data.get(o..o + 4)?.try_into().ok()?);
    o += 4;
    let publish_time = i64::from_le_bytes(data.get(o..o + 8)?.try_into().ok()?);
    o += 8;
    let prev_publish_time = i64::from_le_bytes(data.get(o..o + 8)?.try_into().ok()?);
    o += 8;
    let ema_price = i64::from_le_bytes(data.get(o..o + 8)?.try_into().ok()?);
    o += 8;
    let ema_conf = u64::from_le_bytes(data.get(o..o + 8)?.try_into().ok()?);
    o += 8;
    let posted_slot = u64::from_le_bytes(data.get(o..o + 8)?.try_into().ok()?);
    Some(SolUsdPriceUpdate {
        price,
        conf,
        exponent,
        publish_time,
        prev_publish_time,
        ema_price,
        ema_conf,
        posted_slot,
        full_verification,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../fixtures/pyth_sol_usd_price_update_v2.hex");

    fn fixture_bytes() -> Vec<u8> {
        let hex = FIXTURE.trim();
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn decodes_real_price_update_v2() {
        let d = fixture_bytes();
        let u = decode_sol_usd_price_update(&d).expect("decode");
        assert!(u.full_verification);
        assert_eq!(u.exponent, -8);
        assert_eq!(u.price, 11_197_630_467);
        assert_eq!(u.conf, 4_419_533);
        assert_eq!(u.publish_time, 1_791_466_553);
        assert_eq!(u.prev_publish_time, 1_791_466_552);
        assert_eq!(u.posted_slot, 454_559_902);
        // $111.976304 exactly (micros, floored), integer-only.
        assert_eq!(u.price_micros(), Some(111_976_304));
        assert_eq!(u.conf_micros(), Some(44_195));
    }

    #[test]
    fn wrong_discriminator_rejected() {
        let mut d = fixture_bytes();
        d[0] ^= 0xff;
        assert!(decode_sol_usd_price_update(&d).is_none());
    }

    #[test]
    fn wrong_feed_id_rejected() {
        let mut d = fixture_bytes();
        d[60] ^= 0xff; // inside feed_id
        assert!(decode_sol_usd_price_update(&d).is_none());
    }

    #[test]
    fn truncated_rejected() {
        let d = fixture_bytes();
        assert!(decode_sol_usd_price_update(&d[..80]).is_none());
    }

    #[test]
    fn scaled_conversions() {
        let mk = |price: i64, exponent: i32, full: bool| SolUsdPriceUpdate {
            price,
            conf: 0,
            exponent,
            publish_time: 0,
            prev_publish_time: 0,
            ema_price: 0,
            ema_conf: 0,
            posted_slot: 0,
            full_verification: full,
        };
        // exponent -8: price/100.
        assert_eq!(
            mk(20_000_000_000, -8, true).price_micros(),
            Some(200_000_000)
        ); // $200
           // exponent -6: identity.
        assert_eq!(mk(2_000_000, -6, true).price_micros(), Some(2_000_000));
        // exponent 0: multiply into micros (price 2 -> $2).
        assert_eq!(mk(2, 0, true).price_micros(), Some(2_000_000));
        // positive exponent (price 1e1 -> $10).
        assert_eq!(mk(1, 1, true).price_micros(), Some(10_000_000));
        // zero / negative price -> None.
        assert_eq!(mk(0, -8, true).price_micros(), None);
        assert_eq!(mk(-5, -8, true).price_micros(), None);
        // partial verification -> None (fail closed).
        assert_eq!(mk(20_000_000_000, -8, false).price_micros(), None);
        // overflow -> None.
        assert_eq!(mk(i64::MAX, 30, true).price_micros(), None);
        // sub-micro exponent floors.
        assert_eq!(mk(123_456_789_012, -12, true).price_micros(), Some(123_456));
    }
}
