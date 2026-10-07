//! Incremental per-market state.
//!
//! State is *continuous*: Neurone never rebuilds a market from scratch for
//! each event. Each event mutates the state it already owns.
//!
//! Only fields that Milestone 1 can genuinely derive from the stream are
//! modelled. Price/liquidity/volume require AMM instruction decoding and
//! belong to a later milestone; inventing them here would be dishonest.

use crate::events::{AccountUpdate, MarketKey, TransactionUpdate};

/// Lifecycle status. Milestone 1 only observes; later milestones add
/// qualified/armed/position states.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarketStatus {
    Observing,
}

/// State machine for a single market (one tracked account pubkey).
#[derive(Clone, Debug)]
pub struct MarketState {
    pub key: MarketKey,
    /// Owning program of the tracked account, when known.
    pub owner: Option<MarketKey>,
    pub status: MarketStatus,
    pub last_slot: u64,
    /// Lamports held by the tracked account at the last applied update.
    pub lamports: u64,
    pub data_len: u64,
    /// FNV-1a digest of the account data — a deterministic state fingerprint.
    pub data_digest: u64,
    pub write_version: u64,
    pub account_updates: u64,
    pub tx_count: u64,
    pub last_tx_slot: u64,
    pub last_tx_signature: Option<[u8; 64]>,
    /// Instrumentation only (monotonic ns); excluded from equality.
    pub first_seen_ns: u128,
    pub last_update_ns: u128,
}

impl MarketState {
    /// Create the initial state for a newly observed market account.
    pub fn new(key: MarketKey, now_ns: u128) -> Self {
        Self {
            key,
            owner: None,
            status: MarketStatus::Observing,
            last_slot: 0,
            lamports: 0,
            data_len: 0,
            data_digest: 0,
            write_version: 0,
            account_updates: 0,
            tx_count: 0,
            last_tx_slot: 0,
            last_tx_signature: None,
            first_seen_ns: now_ns,
            last_update_ns: now_ns,
        }
    }

    /// Apply an account update. Returns `false` when the update is stale or a
    /// duplicate (same or older `write_version`), in which case state is
    /// untouched — this is what makes replayed events safe.
    pub fn apply_account(&mut self, ev: &AccountUpdate, now_ns: u128) -> bool {
        if self.account_updates > 0 && ev.write_version <= self.write_version {
            return false;
        }
        self.owner = ev.owner.or(self.owner);
        self.last_slot = self.last_slot.max(ev.slot);
        self.lamports = ev.lamports;
        self.data_len = ev.data_len;
        self.data_digest = ev.data_digest;
        self.write_version = ev.write_version;
        self.account_updates += 1;
        self.last_update_ns = now_ns;
        true
    }

    /// Apply a transaction that touched this market. Returns `false` when the
    /// transaction is an exact replay of the last one observed for this market.
    pub fn apply_transaction(&mut self, ev: &TransactionUpdate, now_ns: u128) -> bool {
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
}

// Deterministic equality: identical event sequences must yield identical
// state. Monotonic timestamps are instrumentation and are therefore excluded.
impl PartialEq for MarketState {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
            && self.owner == other.owner
            && self.status == other.status
            && self.last_slot == other.last_slot
            && self.lamports == other.lamports
            && self.data_len == other.data_len
            && self.data_digest == other.data_digest
            && self.write_version == other.write_version
            && self.account_updates == other.account_updates
            && self.tx_count == other.tx_count
            && self.last_tx_slot == other.last_tx_slot
            && self.last_tx_signature == other.last_tx_signature
    }
}

impl Eq for MarketState {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::AccountUpdate;

    fn update(write_version: u64, digest: u64) -> AccountUpdate {
        AccountUpdate {
            pubkey: MarketKey([1u8; 32]),
            slot: 10,
            owner: Some(MarketKey([2u8; 32])),
            lamports: 100,
            data_len: 8,
            data_digest: digest,
            write_version,
            is_startup: false,
            txn_signature: None,
        }
    }

    #[test]
    fn rejects_stale_and_duplicate_updates() {
        let mut s = MarketState::new(MarketKey([1u8; 32]), 0);
        assert!(s.apply_account(&update(5, 111), 1));
        // Duplicate write_version.
        assert!(!s.apply_account(&update(5, 222), 2));
        // Older write_version.
        assert!(!s.apply_account(&update(4, 333), 3));
        assert_eq!(s.data_digest, 111);
        assert_eq!(s.account_updates, 1);
        // Newer write_version applies.
        assert!(s.apply_account(&update(6, 444), 4));
        assert_eq!(s.data_digest, 444);
    }

    #[test]
    fn transaction_replay_is_idempotent() {
        let mut s = MarketState::new(MarketKey([1u8; 32]), 0);
        let tx = TransactionUpdate {
            signature: [7u8; 64],
            slot: 3,
            index: 0,
            is_vote: false,
            success: true,
            keys: vec![MarketKey([1u8; 32])],
        };
        assert!(s.apply_transaction(&tx, 1));
        assert!(!s.apply_transaction(&tx, 2));
        assert_eq!(s.tx_count, 1);
    }
}
