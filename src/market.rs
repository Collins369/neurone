//! Incremental protocol-derived market state.
//!
//! State is *continuous*: Neurone never rebuilds a market from scratch for each
//! event. Each event mutates the state it already owns.
//!
//! ## Units
//!
//! Everything stored here is in **raw on-chain integer units**:
//! * `base_reserve` / `virtual_base_reserve` — raw base-token units;
//! * `quote_reserve` / `virtual_quote_reserve` — raw quote units (lamports when
//!   the quote mint is wrapped SOL);
//! * volume fields — raw quote/base units.
//!
//! No UI decimals or USD conversion is applied, because decimals live on the
//! mint accounts, which this milestone does not read. Prices are therefore
//! exact integer ratios, never floating point, so state stays deterministic.

use std::collections::VecDeque;

use crate::decode::{CreatedMarket, DecodedAccount, DecodedSwap, Venue};
use crate::events::{AccountUpdate, MarketKey, TransactionUpdate, VaultBalance};

/// Default number of sparse per-slot volume buckets retained per market.
pub const DEFAULT_VOLUME_BUCKETS: usize = 1024;

/// Lifecycle status. M2 only observes; later milestones add qualified/armed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarketStatus {
    Observing,
}

/// Freshness/trust of a market's reserve state (infrastructure concept, not
/// strategy).
///
/// The variants deliberately separate *transient bootstrap* from
/// *untrustworthy* state, because a market observed after launch must not be
/// treated as unusable merely for missing its history:
///
/// * `Unknown` — identity known, but no authoritative reserves observed yet.
///   This is the **normal, expected** state of a market discovered after
///   launch. It clears automatically as soon as the next authoritative
///   Yellowstone update arrives (a bonding-curve account update or a decoded
///   swap) and never requires observing the market's launch or earlier trades.
///   It is not quotable, but it is a transient bootstrap, not a rejection.
/// * `Known` — reserves observed within the configured slot tolerance.
/// * `Stale` — reserves observed, but not recently enough to trust.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReserveState {
    Unknown,
    Known,
    Stale,
    /// A genuine non-trade reserve mutation was observed and the market state
    /// has not yet been re-established from a fresh authoritative source. Never
    /// quotable.
    Invalidated,
}

/// An exact rational price: `num / den` raw quote units per raw base unit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Ratio {
    pub num: u128,
    pub den: u128,
}

impl Ratio {
    pub fn new(num: u128, den: u128) -> Option<Self> {
        (den != 0).then_some(Ratio { num, den })
    }

    /// Convenience conversion for telemetry only; never used for state.
    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }

    /// Scale by `10^base_decimals / 10^quote_decimals` to express the price in
    /// UI units. Kept separate so unit mistakes are explicit.
    pub fn scaled(self, base_decimals: u32, quote_decimals: u32) -> Option<Ratio> {
        let base_scale = 10u128.checked_pow(base_decimals)?;
        let quote_scale = 10u128.checked_pow(quote_decimals)?;
        Ratio::new(
            self.num.checked_mul(base_scale)?,
            self.den.checked_mul(quote_scale)?,
        )
    }
}

/// Per-slot volume aggregate.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SlotVolume {
    pub slot: u64,
    pub buy_quote: u64,
    pub sell_quote: u64,
    pub buy_base: u64,
    pub sell_base: u64,
    pub trades: u32,
}

/// Summed volume over a slot range.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct VolumeTotals {
    pub buy_quote: u64,
    pub sell_quote: u64,
    pub buy_base: u64,
    pub sell_base: u64,
    pub trades: u64,
}

impl VolumeTotals {
    pub fn total_quote(&self) -> u64 {
        self.buy_quote.saturating_add(self.sell_quote)
    }
}

/// Bounded, sparse, per-slot volume window.
///
/// Only slots that actually traded get a bucket, so an idle market costs
/// nothing. Oldest buckets are evicted once `capacity` is reached, which keeps
/// per-market memory bounded no matter how long the process runs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct VolumeWindow {
    capacity: usize,
    buckets: VecDeque<SlotVolume>,
}

impl VolumeWindow {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            buckets: VecDeque::new(),
        }
    }

    /// Record a trade. Slots are expected non-decreasing per market.
    pub fn record(&mut self, slot: u64, is_buy: bool, quote: u64, base: u64) {
        let bucket = match self.buckets.back_mut() {
            Some(b) if b.slot == slot => b,
            _ => {
                // Out-of-order (rare): reuse an existing bucket if present.
                if let Some(idx) = self.buckets.iter().rposition(|b| b.slot == slot) {
                    &mut self.buckets[idx]
                } else {
                    self.buckets.push_back(SlotVolume {
                        slot,
                        ..Default::default()
                    });
                    self.buckets.back_mut().expect("just pushed")
                }
            }
        };
        if is_buy {
            bucket.buy_quote = bucket.buy_quote.saturating_add(quote);
            bucket.buy_base = bucket.buy_base.saturating_add(base);
        } else {
            bucket.sell_quote = bucket.sell_quote.saturating_add(quote);
            bucket.sell_base = bucket.sell_base.saturating_add(base);
        }
        bucket.trades = bucket.trades.saturating_add(1);
        while self.buckets.len() > self.capacity {
            self.buckets.pop_front();
        }
    }

    /// Total volume recorded in buckets with `slot >= from_slot`.
    pub fn totals_since(&self, from_slot: u64) -> VolumeTotals {
        let mut t = VolumeTotals::default();
        for b in self.buckets.iter().rev() {
            if b.slot < from_slot {
                break;
            }
            t.buy_quote = t.buy_quote.saturating_add(b.buy_quote);
            t.sell_quote = t.sell_quote.saturating_add(b.sell_quote);
            t.buy_base = t.buy_base.saturating_add(b.buy_base);
            t.sell_base = t.sell_base.saturating_add(b.sell_base);
            t.trades += u64::from(b.trades);
        }
        t
    }

    /// Total volume across the retained window.
    pub fn totals(&self) -> VolumeTotals {
        self.totals_since(0)
    }

    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }
}

/// State machine for a single market.
#[derive(Clone, Debug)]
pub struct MarketState {
    pub key: MarketKey,
    pub venue: Option<Venue>,
    pub status: MarketStatus,

    // Identity.
    pub base_mint: Option<MarketKey>,
    pub quote_mint: Option<MarketKey>,
    pub owner: Option<MarketKey>,
    pub creator: Option<MarketKey>,
    pub pool_base_token_account: Option<MarketKey>,
    pub pool_quote_token_account: Option<MarketKey>,

    // Chain position / account bookkeeping.
    pub last_slot: u64,
    pub lamports: u64,
    pub data_len: u64,
    pub data_digest: u64,
    pub write_version: u64,
    pub account_updates: u64,
    pub tx_count: u64,
    pub last_tx_slot: u64,
    pub last_tx_signature: Option<[u8; 64]>,
    pub decode_failures: u64,

    // Protocol-derived reserves (raw units).
    pub base_reserve: u128,
    pub quote_reserve: u128,
    pub virtual_base_reserve: u128,
    pub virtual_quote_reserve: i128,
    pub token_total_supply: u128,
    pub complete: bool,

    // Trade activity.
    pub trade_count: u64,
    pub buy_count: u64,
    pub sell_count: u64,
    pub last_trade_slot: u64,
    pub last_trade_timestamp: Option<i64>,
    pub last_fee_bps: Option<u64>,
    pub volume: VolumeWindow,

    // Authoritative reserve provenance (raw units are in base/quote_reserve).
    /// True once an authoritative source (bonding-curve account or swap event)
    /// has supplied reserves.
    pub reserves_known: bool,
    pub last_reserve_slot: u64,
    pub last_reserve_timestamp: Option<i64>,
    pub last_reserve_signature: Option<[u8; 64]>,
    /// Monotonic per-market state version; bumped on every applied change and
    /// on invalidation. An armed trade must record the version it armed from.
    pub state_version: u64,
    /// Slot of the most recent non-trade reserve mutation that invalidated the
    /// state, if any. Cleared by a fresh account update at `slot >= this`.
    pub invalidated_at_slot: Option<u64>,

    // Instrumentation only (monotonic ns); excluded from equality.
    pub first_seen_ns: u128,
    pub last_update_ns: u128,
}

impl MarketState {
    /// Create the initial state for a newly observed market.
    pub fn new(key: MarketKey, now_ns: u128) -> Self {
        Self {
            key,
            venue: None,
            status: MarketStatus::Observing,
            base_mint: None,
            quote_mint: None,
            owner: None,
            creator: None,
            pool_base_token_account: None,
            pool_quote_token_account: None,
            last_slot: 0,
            lamports: 0,
            data_len: 0,
            data_digest: 0,
            write_version: 0,
            account_updates: 0,
            tx_count: 0,
            last_tx_slot: 0,
            last_tx_signature: None,
            decode_failures: 0,
            base_reserve: 0,
            quote_reserve: 0,
            virtual_base_reserve: 0,
            virtual_quote_reserve: 0,
            token_total_supply: 0,
            complete: false,
            trade_count: 0,
            buy_count: 0,
            sell_count: 0,
            last_trade_slot: 0,
            last_trade_timestamp: None,
            last_fee_bps: None,
            volume: VolumeWindow::new(DEFAULT_VOLUME_BUCKETS),
            reserves_known: false,
            last_reserve_slot: 0,
            last_reserve_timestamp: None,
            last_reserve_signature: None,
            state_version: 0,
            invalidated_at_slot: None,
            first_seen_ns: now_ns,
            last_update_ns: now_ns,
        }
    }

    /// Apply an account update. Returns `false` for a stale/duplicate update
    /// (same or older `write_version`), leaving state untouched.
    pub fn apply_account(&mut self, ev: &AccountUpdate, now_ns: u128) -> bool {
        if self.account_updates > 0 && ev.write_version <= self.write_version {
            return false;
        }
        // A fresh authoritative account update at or after an invalidation
        // re-establishes valid state (deterministic by the account's own slot).
        if let Some(inv) = self.invalidated_at_slot {
            if ev.slot >= inv {
                self.invalidated_at_slot = None;
            }
        }
        self.owner = ev.owner.or(self.owner);
        self.last_slot = self.last_slot.max(ev.slot);
        self.lamports = ev.lamports;
        self.data_len = ev.data_len;
        self.data_digest = ev.data_digest;
        self.write_version = ev.write_version;
        self.account_updates += 1;
        self.last_update_ns = now_ns;
        if let Some(decoded) = &ev.decoded {
            self.apply_decoded_account(decoded);
        }
        self.state_version = self.state_version.wrapping_add(1);
        true
    }

    /// Mark the market state invalid after a non-trade reserve mutation at
    /// `slot`. The state must not be quoted until re-established by a fresh
    /// authoritative update at a slot `>= slot`.
    pub fn invalidate(&mut self, slot: u64) {
        self.invalidated_at_slot = Some(match self.invalidated_at_slot {
            Some(s) => s.max(slot),
            None => slot,
        });
        self.state_version = self.state_version.wrapping_add(1);
        self.last_update_ns = crate::clock::now_ns();
    }

    fn apply_decoded_account(&mut self, d: &DecodedAccount) {
        self.venue = Some(d.venue);
        self.base_mint = d.base_mint.or(self.base_mint);
        self.quote_mint = d.quote_mint.or(self.quote_mint);
        self.creator = d.creator.or(self.creator);
        self.pool_base_token_account = d.pool_base_token_account.or(self.pool_base_token_account);
        self.pool_quote_token_account =
            d.pool_quote_token_account.or(self.pool_quote_token_account);
        // Only sources that carry real reserves mark the state known.
        if let (Some(b), Some(q)) = (d.base_reserve, d.quote_reserve) {
            self.base_reserve = b;
            self.quote_reserve = q;
            self.reserves_known = true;
            self.last_reserve_slot = self.last_reserve_slot.max(self.last_slot);
        } else {
            if let Some(v) = d.base_reserve {
                self.base_reserve = v;
            }
            if let Some(v) = d.quote_reserve {
                self.quote_reserve = v;
            }
        }
        if let Some(v) = d.virtual_base_reserve {
            self.virtual_base_reserve = v;
        }
        if let Some(v) = d.virtual_quote_reserve {
            self.virtual_quote_reserve = v;
        }
        if let Some(v) = d.token_total_supply {
            self.token_total_supply = v;
        }
        if let Some(v) = d.complete {
            self.complete = v;
        }
    }

    /// Record a transaction that touched this market (no swap decoded).
    pub fn apply_touch(&mut self, ev: &TransactionUpdate, now_ns: u128) -> bool {
        if self.last_tx_signature == Some(ev.signature) {
            return false;
        }
        self.tx_count += 1;
        self.last_tx_slot = self.last_tx_slot.max(ev.slot);
        self.last_slot = self.last_slot.max(ev.slot);
        self.last_tx_signature = Some(ev.signature);
        self.last_update_ns = now_ns;
        true
    }

    /// Advance the market's slot watermark (used before applying swaps so the
    /// volume bucket lands in the correct slot).
    pub fn note_slot(&mut self, slot: u64) {
        self.last_slot = self.last_slot.max(slot);
    }

    /// Whether the current reserve state is fresh enough to quote from.
    pub fn reserve_state(&self, current_slot: u64, stale_slots: u64) -> ReserveState {
        if self.invalidated_at_slot.is_some() {
            ReserveState::Invalidated
        } else if !self.reserves_known {
            ReserveState::Unknown
        } else if current_slot.saturating_sub(self.last_reserve_slot) > stale_slots {
            ReserveState::Stale
        } else {
            ReserveState::Known
        }
    }

    /// Convenience freshness predicate.
    pub fn is_reserve_state_fresh(&self, current_slot: u64, stale_slots: u64) -> bool {
        self.reserve_state(current_slot, stale_slots) == ReserveState::Known
    }

    /// Seed identity/reserves from a `CreateEvent`.
    pub fn apply_create(&mut self, c: &CreatedMarket, now_ns: u128) {
        self.venue.get_or_insert(c.venue);
        self.base_mint = Some(c.mint).or(self.base_mint);
        self.quote_mint = c.quote_mint.or(self.quote_mint);
        if let Some(v) = c.virtual_base_reserve {
            self.virtual_base_reserve = v;
        }
        if let Some(v) = c.virtual_quote_reserve {
            self.virtual_quote_reserve = v;
        }
        if let Some(v) = c.base_reserve {
            self.base_reserve = v;
        }
        if let Some(v) = c.token_total_supply {
            self.token_total_supply = v;
        }
        if let Some(ts) = c.timestamp {
            self.last_trade_timestamp = Some(ts);
        }
        self.last_update_ns = now_ns;
    }

    /// Apply a decoded swap. Returns `false` if it is an exact replay.
    pub fn apply_swap(
        &mut self,
        s: &DecodedSwap,
        signature: [u8; 64],
        slot: u64,
        now_ns: u128,
    ) -> bool {
        self.last_slot = self.last_slot.max(slot);
        self.venue.get_or_insert(s.venue);
        self.base_mint = s.base_mint.or(self.base_mint);
        self.quote_mint = s.quote_mint.or(self.quote_mint);

        // Reserve freshness: an event supplies the latest observed pool
        // reserves. An older event must never overwrite newer reserve state.
        if let (Some(b), Some(q)) = (s.base_reserve, s.quote_reserve) {
            if !self.reserves_known || slot >= self.last_reserve_slot {
                self.base_reserve = u128::from(b);
                self.quote_reserve = u128::from(q);
                self.reserves_known = true;
                self.last_reserve_slot = slot;
                self.last_reserve_timestamp = s.timestamp;
                self.last_reserve_signature = Some(signature);
            }
        }
        if let Some(v) = s.virtual_quote_reserve {
            self.virtual_quote_reserve = v;
        }
        if let Some(ts) = s.timestamp {
            self.last_trade_timestamp = Some(ts);
        }
        if let Some(bps) = s.fee_bps {
            self.last_fee_bps = Some(bps);
        }

        self.trade_count += 1;
        if s.is_buy {
            self.buy_count += 1;
        } else {
            self.sell_count += 1;
        }
        self.volume
            .record(self.last_slot, s.is_buy, s.quote_amount, s.base_amount);
        self.last_trade_slot = self.last_slot;
        self.last_update_ns = now_ns;
        true
    }

    /// Establish/refresh a PumpSwap pool's reserves from transaction
    /// `post_token_balances` and the pool's `virtual_quote_reserves`.
    ///
    /// A pool's raw balances live in its two SPL token vaults, which the `Pool`
    /// account does not restate. When a transaction's authoritative
    /// post-balances include both of this pool's configured vaults — matched by
    /// **exact vault pubkey** and **expected mint** — reserves become:
    ///
    /// ```text
    /// base_reserve  = raw base vault balance
    /// quote_reserve = raw quote vault balance + virtual_quote_reserves (i128)
    /// ```
    ///
    /// This is the late-observation bootstrap: it does **not** require a decoded
    /// swap, the pool's launch, or a predecessor chain. It never fabricates a
    /// reserve: unless the pool metadata is known and both vault balances are
    /// present, consistently matched, and the effective quote reserve is
    /// non-negative, it leaves the market untouched (still `Unknown`). Older
    /// transactions cannot overwrite newer reserve state.
    ///
    /// Returns `true` when reserves were established/updated.
    pub fn apply_vault_balances(
        &mut self,
        balances: &[VaultBalance],
        slot: u64,
        signature: [u8; 64],
        now_ns: u128,
    ) -> bool {
        // Only a PumpSwap pool carries the vault metadata + signed virtual
        // quote reserve needed to compose effective reserves.
        let (Some(base_vault), Some(quote_vault)) =
            (self.pool_base_token_account, self.pool_quote_token_account)
        else {
            return false;
        };
        let (Some(base_mint), Some(quote_mint)) = (self.base_mint, self.quote_mint) else {
            return false;
        };

        // Positive association: exact vault pubkey, expected mint, and the
        // vault's authority must be this pool (so a balance from an unrelated
        // account can never contaminate the pool).
        let mut base_raw = None;
        let mut quote_raw = None;
        for b in balances {
            if b.authority != self.key {
                continue;
            }
            if b.account == base_vault && b.mint == base_mint {
                base_raw = Some(b.amount);
            } else if b.account == quote_vault && b.mint == quote_mint {
                quote_raw = Some(b.amount);
            }
        }
        let (Some(base_raw), Some(quote_raw)) = (base_raw, quote_raw) else {
            return false;
        };

        // Effective quote reserve: raw balance + signed virtual reserve, exact
        // integer arithmetic, no floats.
        let Some(effective_quote) = (quote_raw as i128).checked_add(self.virtual_quote_reserve)
        else {
            return false;
        };
        if effective_quote < 0 {
            return false;
        }

        // Never regress newer authoritative reserve state.
        if self.reserves_known && slot < self.last_reserve_slot {
            return false;
        }

        self.base_reserve = u128::from(base_raw);
        self.quote_reserve = effective_quote as u128;
        self.reserves_known = true;
        self.last_reserve_slot = slot;
        self.last_reserve_timestamp = None;
        self.last_reserve_signature = Some(signature);
        self.last_slot = self.last_slot.max(slot);
        self.last_update_ns = now_ns;
        true
    }

    /// Reference (spot) price from pool/curve reserves: quote per base, raw.
    pub fn spot_price_raw(&self) -> Option<Ratio> {
        let base = self.virtual_base_reserve.max(self.base_reserve);
        let quote = if self.virtual_quote_reserve > 0 {
            self.virtual_quote_reserve as u128
        } else {
            self.quote_reserve
        };
        if base == 0 || quote == 0 {
            // A zero side means there is no meaningful price, not a zero price.
            return None;
        }
        Ratio::new(quote, base)
    }

    /// Executable constant-product price for a given input size.
    ///
    /// This is the average price actually realised by a trade of `amount`, not
    /// the marginal reserve ratio: it applies the constant-product impact and
    /// the fee. `amount` is quote-in for a buy and base-in for a sell.
    /// Returns the raw quote-per-base price.
    pub fn executable_price(&self, is_buy: bool, amount: u64, fee_bps: u64) -> Option<Ratio> {
        let x = self.virtual_base_reserve; // base
        let y = if self.virtual_quote_reserve > 0 {
            self.virtual_quote_reserve as u128
        } else {
            self.quote_reserve
        };
        if x == 0 || y == 0 || amount == 0 {
            return None;
        }
        let bps = 10_000u128;
        let fee_bps = u128::from(fee_bps.min(10_000));
        let amount = u128::from(amount);
        if is_buy {
            let net = amount.checked_mul(bps - fee_bps)? / bps;
            let tokens_out = x.checked_mul(net)? / y.checked_add(net)?;
            Ratio::new(amount, tokens_out)
        } else {
            let tokens_out = y.checked_mul(amount)? / x.checked_add(amount)?;
            let net = tokens_out.checked_mul(bps - fee_bps)? / bps;
            Ratio::new(net, amount)
        }
    }
}

// Deterministic equality: identical event sequences must yield identical state.
// Monotonic timestamps are instrumentation and are therefore excluded.
impl PartialEq for MarketState {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
            && self.venue == other.venue
            && self.status == other.status
            && self.base_mint == other.base_mint
            && self.quote_mint == other.quote_mint
            && self.owner == other.owner
            && self.creator == other.creator
            && self.pool_base_token_account == other.pool_base_token_account
            && self.pool_quote_token_account == other.pool_quote_token_account
            && self.last_slot == other.last_slot
            && self.lamports == other.lamports
            && self.data_len == other.data_len
            && self.data_digest == other.data_digest
            && self.write_version == other.write_version
            && self.account_updates == other.account_updates
            && self.tx_count == other.tx_count
            && self.last_tx_slot == other.last_tx_slot
            && self.last_tx_signature == other.last_tx_signature
            && self.decode_failures == other.decode_failures
            && self.base_reserve == other.base_reserve
            && self.quote_reserve == other.quote_reserve
            && self.virtual_base_reserve == other.virtual_base_reserve
            && self.virtual_quote_reserve == other.virtual_quote_reserve
            && self.token_total_supply == other.token_total_supply
            && self.complete == other.complete
            && self.trade_count == other.trade_count
            && self.buy_count == other.buy_count
            && self.sell_count == other.sell_count
            && self.last_trade_slot == other.last_trade_slot
            && self.last_trade_timestamp == other.last_trade_timestamp
            && self.last_fee_bps == other.last_fee_bps
            && self.volume == other.volume
            && self.reserves_known == other.reserves_known
            && self.last_reserve_slot == other.last_reserve_slot
            && self.last_reserve_timestamp == other.last_reserve_timestamp
            && self.last_reserve_signature == other.last_reserve_signature
            && self.state_version == other.state_version
            && self.invalidated_at_slot == other.invalidated_at_slot
    }
}

impl Eq for MarketState {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::AccountUpdate;

    fn key(b: u8) -> MarketKey {
        MarketKey([b; 32])
    }

    fn update(write_version: u64, digest: u64) -> AccountUpdate {
        AccountUpdate {
            pubkey: key(1),
            slot: 10,
            owner: Some(key(2)),
            lamports: 100,
            data_len: 8,
            data_digest: digest,
            write_version,
            is_startup: false,
            txn_signature: None,
            decoded: None,
        }
    }

    #[test]
    fn rejects_stale_and_duplicate_updates() {
        let mut s = MarketState::new(key(1), 0);
        assert!(s.apply_account(&update(5, 111), 1));
        assert!(!s.apply_account(&update(5, 222), 2));
        assert!(!s.apply_account(&update(4, 333), 3));
        assert_eq!(s.data_digest, 111);
        assert_eq!(s.account_updates, 1);
        assert!(s.apply_account(&update(6, 444), 4));
        assert_eq!(s.data_digest, 444);
    }

    #[test]
    fn volume_window_accumulates_and_bounds() {
        let mut w = VolumeWindow::new(3);
        w.record(1, true, 100, 10);
        w.record(1, false, 50, 5);
        w.record(2, true, 200, 20);
        assert_eq!(w.len(), 2);
        let t = w.totals();
        assert_eq!(t.buy_quote, 300);
        assert_eq!(t.sell_quote, 50);
        assert_eq!(t.trades, 3);
        // Eviction keeps at most `capacity` buckets.
        w.record(3, true, 1, 1);
        w.record(4, true, 1, 1);
        assert!(w.len() <= 3);
        // Windowed query.
        assert_eq!(w.totals_since(3).buy_quote, 2);
    }

    #[test]
    fn price_is_exact_integer_ratio() {
        let mut s = MarketState::new(key(1), 0);
        s.virtual_base_reserve = 1_000;
        s.virtual_quote_reserve = 2_000;
        assert_eq!(s.spot_price_raw(), Ratio::new(2_000, 1_000));
        // A buy of 100 quote with 0 fee: net 100, tokens = 1000*100/2100 = 47.
        let p = s.executable_price(true, 100, 0).unwrap();
        assert_eq!(p.den, 47);
        assert_eq!(p.num, 100);
        // Executable price is worse (higher) than spot for a buy.
        assert!(p.as_f64() > s.spot_price_raw().unwrap().as_f64());
    }

    #[test]
    fn market_state_size_is_bounded() {
        // Guards against accidental unbounded growth of the hot-path state.
        let size = std::mem::size_of::<MarketState>();
        println!("size_of::<MarketState>() = {size} bytes");
        assert!(size <= 768, "MarketState grew unexpectedly: {size} bytes");
    }
}
