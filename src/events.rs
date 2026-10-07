//! Compact internal event representation.
//!
//! Yellowstone protobuf payloads are large and reference-rich. They are
//! normalized exactly once, at the ingestion boundary, into owned, bounded
//! values. Downstream stages never touch protobuf types, which keeps the hot
//! path small and makes replay tests deterministic.

use std::fmt;

use base64::Engine as _;
use yellowstone_grpc_proto::prelude::{subscribe_update::UpdateOneof, SubscribeUpdate};

use crate::decode::{self, CreatedMarket, DecodedAccount, DecodedSwap};
use crate::hash::fnv1a_64;

/// A market identity: the 32-byte account pubkey that owns a market's state.
pub const PUBKEY_LEN: usize = 32;
/// A Solana transaction signature.
pub const SIGNATURE_LEN: usize = 64;
/// Upper bound on the number of account keys we carry from a transaction.
///
/// A Solana message is bounded, but we cap defensively so a malformed or
/// unusual message cannot inflate per-event memory.
pub const MAX_TX_KEYS: usize = 64;
/// Upper bound on decoded protocol events carried per transaction.
pub const MAX_DECODED_EVENTS: usize = 16;
/// Maximum decoded size of a `Program data:` payload we will consider.
/// pump.swap swap events are ~450-500 bytes, so this must comfortably exceed
/// them; anything larger is treated as not-our-event.
const MAX_PROGRAM_DATA_LEN: usize = 2_048;
const PROGRAM_DATA_PREFIX: &str = "Program data: ";

/// Deterministic market identity.
///
/// It is a `[u8; 32]` account pubkey. Ordering, hashing and (later) shard
/// routing all derive from the raw bytes so identity is stable everywhere.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MarketKey(pub [u8; PUBKEY_LEN]);

impl MarketKey {
    /// Build a key from a byte slice, requiring exactly 32 bytes.
    #[inline]
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        if bytes.len() == PUBKEY_LEN {
            let mut out = [0u8; PUBKEY_LEN];
            out.copy_from_slice(bytes);
            Some(MarketKey(out))
        } else {
            None
        }
    }

    /// Borrow the raw bytes.
    #[inline]
    pub fn as_bytes(&self) -> &[u8; PUBKEY_LEN] {
        &self.0
    }

    /// Parse a base58 pubkey.
    pub fn from_base58(s: &str) -> Option<Self> {
        Self::from_slice(&bs58::decode(s).into_vec().ok()?)
    }

    /// Base58 rendering, for logs only (never in the hot path).
    pub fn to_base58(&self) -> String {
        bs58::encode(self.0).into_string()
    }
}

impl fmt::Debug for MarketKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MarketKey({})", self.to_base58())
    }
}

impl fmt::Display for MarketKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_base58())
    }
}

/// Slot lifecycle status, mirroring `geyser::SlotStatus`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlotStatusKind {
    Processed,
    Confirmed,
    Finalized,
    FirstShredReceived,
    Completed,
    CreatedBank,
    Dead,
    Unknown(i32),
}

impl SlotStatusKind {
    fn from_i32(v: i32) -> Self {
        use yellowstone_grpc_proto::prelude::SlotStatus;
        match v {
            x if x == SlotStatus::SlotProcessed as i32 => SlotStatusKind::Processed,
            x if x == SlotStatus::SlotConfirmed as i32 => SlotStatusKind::Confirmed,
            x if x == SlotStatus::SlotFinalized as i32 => SlotStatusKind::Finalized,
            x if x == SlotStatus::SlotFirstShredReceived as i32 => {
                SlotStatusKind::FirstShredReceived
            }
            x if x == SlotStatus::SlotCompleted as i32 => SlotStatusKind::Completed,
            x if x == SlotStatus::SlotCreatedBank as i32 => SlotStatusKind::CreatedBank,
            x if x == SlotStatus::SlotDead as i32 => SlotStatusKind::Dead,
            other => SlotStatusKind::Unknown(other),
        }
    }
}

/// A normalized event plus ingestion timestamps.
#[derive(Clone, Debug)]
pub struct NormalizedEvent {
    pub kind: EventKind,
    /// Monotonic timestamp (ns) when the ingestion loop received the update.
    pub arrival_ns: u128,
    /// Server-side `created_at` timestamp, when the server supplied one.
    pub server_created_at_ns: Option<u128>,
}

impl NormalizedEvent {
    /// Construct an event with a fresh arrival timestamp.
    pub fn new(kind: EventKind) -> Self {
        Self {
            kind,
            arrival_ns: crate::clock::now_ns(),
            server_created_at_ns: None,
        }
    }
}

/// The bounded set of events Neurone reacts to in Milestone 1.
// Account/Transaction carry decoded protocol state inline to avoid an
// allocation per event on the hot path; the size skew is intentional.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventKind {
    Slot(SlotUpdate),
    Account(AccountUpdate),
    Transaction(TransactionUpdate),
    BlockMeta(BlockMetaUpdate),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotUpdate {
    pub slot: u64,
    pub parent: Option<u64>,
    pub status: SlotStatusKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountUpdate {
    pub pubkey: MarketKey,
    pub slot: u64,
    pub owner: Option<MarketKey>,
    pub lamports: u64,
    pub data_len: u64,
    /// FNV-1a digest of the account data. A cheap, deterministic state
    /// fingerprint; full AMM layout decoding is a later milestone.
    pub data_digest: u64,
    pub write_version: u64,
    pub is_startup: bool,
    pub txn_signature: Option<[u8; SIGNATURE_LEN]>,
    /// Protocol-decoded account state, when the owner program is known.
    pub decoded: Option<DecodedAccount>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransactionUpdate {
    pub signature: [u8; SIGNATURE_LEN],
    pub slot: u64,
    pub index: u64,
    pub is_vote: bool,
    pub success: bool,
    /// Account keys touched by the transaction, bounded and deduplicated.
    pub keys: Vec<MarketKey>,
    /// Protocol swap/trade events decoded from program logs.
    pub swaps: Vec<DecodedSwap>,
    /// Markets created by this transaction.
    pub creates: Vec<CreatedMarket>,
    /// Known event payloads that failed to decode (malformed/truncated).
    pub decode_rejected: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockMetaUpdate {
    pub slot: u64,
    pub block_height: Option<u64>,
    pub block_time: Option<i64>,
    pub executed_transaction_count: u64,
}

/// Result of normalizing one `SubscribeUpdate`.
// `Event` is large because it owns the normalized payload; boxing it would add
// an allocation to the hot path for no benefit. `Keepalive`/`Ignored` are rare.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum Normalized {
    Event(NormalizedEvent),
    /// Keepalive (`ping`/`pong`): counted, but not routed.
    Keepalive,
    /// A subscription we do not consume in this milestone.
    Ignored,
}

/// Convert one Yellowstone update into a Neurone event.
///
/// Determinism: the output depends only on the input bytes. Timestamps come
/// from `arrival_ns` (supplied by the caller) and `created_at` (from the
/// server), so two normalizations of the same update are equal.
pub fn normalize(update: &SubscribeUpdate, arrival_ns: u128) -> Normalized {
    let server_created_at_ns = update.created_at.as_ref().and_then(timestamp_to_ns);
    let kind = match update.update_oneof.as_ref() {
        Some(UpdateOneof::Account(a)) => match normalize_account(a) {
            Some(k) => EventKind::Account(k),
            None => return Normalized::Ignored,
        },
        Some(UpdateOneof::Slot(s)) => EventKind::Slot(SlotUpdate {
            slot: s.slot,
            parent: s.parent,
            status: SlotStatusKind::from_i32(s.status),
        }),
        Some(UpdateOneof::Transaction(t)) => match normalize_transaction(t) {
            Some(k) => EventKind::Transaction(k),
            None => return Normalized::Ignored,
        },
        Some(UpdateOneof::BlockMeta(b)) => EventKind::BlockMeta(BlockMetaUpdate {
            slot: b.slot,
            block_height: b.block_height.map(|h| h.block_height),
            block_time: b.block_time.map(|t| t.timestamp),
            executed_transaction_count: b.executed_transaction_count,
        }),
        Some(UpdateOneof::Ping(_)) | Some(UpdateOneof::Pong(_)) => return Normalized::Keepalive,
        // Blocks, entries and transaction-status are not consumed in M1.
        _ => return Normalized::Ignored,
    };
    Normalized::Event(NormalizedEvent {
        kind,
        arrival_ns,
        server_created_at_ns,
    })
}

fn normalize_account(
    a: &yellowstone_grpc_proto::prelude::SubscribeUpdateAccount,
) -> Option<AccountUpdate> {
    let info = a.account.as_ref()?;
    let pubkey = MarketKey::from_slice(&info.pubkey)?;
    let owner = MarketKey::from_slice(&info.owner);
    Some(AccountUpdate {
        pubkey,
        slot: a.slot,
        owner,
        lamports: info.lamports,
        data_len: info.data.len() as u64,
        data_digest: fnv1a_64(&info.data),
        write_version: info.write_version,
        is_startup: a.is_startup,
        txn_signature: info.txn_signature.as_deref().and_then(to_signature),
        decoded: owner.and_then(|o| decode::decode_account(&o, &info.data)),
    })
}

fn normalize_transaction(
    t: &yellowstone_grpc_proto::prelude::SubscribeUpdateTransaction,
) -> Option<TransactionUpdate> {
    let info = t.transaction.as_ref()?;
    let signature = to_signature(&info.signature)?;

    let mut keys: Vec<MarketKey> = Vec::with_capacity(MAX_TX_KEYS);
    if let Some(tx) = info.transaction.as_ref() {
        if let Some(message) = tx.message.as_ref() {
            for k in &message.account_keys {
                push_key(&mut keys, k);
            }
        }
    }
    if let Some(meta) = info.meta.as_ref() {
        for k in meta
            .loaded_writable_addresses
            .iter()
            .chain(&meta.loaded_readonly_addresses)
        {
            push_key(&mut keys, k);
        }
    }

    let (swaps, creates, decode_rejected) = decode_transaction_payloads(info.meta.as_ref());
    for swap in &swaps {
        keys.push(swap.market_key);
    }
    for created in &creates {
        keys.push(created.market_key);
    }
    keys.sort_unstable();
    keys.dedup();

    Some(TransactionUpdate {
        signature,
        slot: t.slot,
        index: info.index,
        is_vote: info.is_vote,
        success: info.meta.as_ref().is_none_or(|m| m.err.is_none()),
        keys,
        swaps,
        creates,
        decode_rejected,
    })
}

/// Decode protocol events from a transaction's program logs.
///
/// Bounded and allocation-light: only recognizable Anchor event payloads are
/// decoded, at most [`MAX_DECODED_EVENTS`] per transaction.
fn decode_transaction_payloads(
    meta: Option<&yellowstone_grpc_proto::solana::storage::confirmed_block::TransactionStatusMeta>,
) -> (Vec<DecodedSwap>, Vec<CreatedMarket>, u32) {
    let mut swaps = Vec::new();
    let mut creates = Vec::new();
    let mut rejected = 0u32;
    let Some(meta) = meta else {
        return (swaps, creates, rejected);
    };
    for line in &meta.log_messages {
        if swaps.len() + creates.len() >= MAX_DECODED_EVENTS {
            break;
        }
        let Some(b64) = line.strip_prefix(PROGRAM_DATA_PREFIX) else {
            continue;
        };
        // Cheap pre-filter before allocating the base64 decode (~1.34x).
        if b64.len() > MAX_PROGRAM_DATA_LEN * 2 {
            continue;
        }
        let Ok(payload) = base64::engine::general_purpose::STANDARD.decode(b64) else {
            continue;
        };
        if payload.len() > MAX_PROGRAM_DATA_LEN {
            continue;
        }
        if let Some(swap) = decode::decode_event(&payload) {
            swaps.push(swap);
        } else if let Some(created) = decode::decode_create_event(&payload) {
            creates.push(created);
        } else if decode::is_known_event_discriminator(&payload) {
            rejected += 1;
        }
    }
    (swaps, creates, rejected)
}

fn push_key(keys: &mut Vec<MarketKey>, bytes: &[u8]) {
    if keys.len() >= MAX_TX_KEYS {
        return;
    }
    if let Some(k) = MarketKey::from_slice(bytes) {
        keys.push(k);
    }
}

fn to_signature(bytes: &[u8]) -> Option<[u8; SIGNATURE_LEN]> {
    if bytes.len() != SIGNATURE_LEN {
        return None;
    }
    let mut out = [0u8; SIGNATURE_LEN];
    out.copy_from_slice(bytes);
    Some(out)
}

fn timestamp_to_ns(ts: &yellowstone_grpc_proto::prost_types::Timestamp) -> Option<u128> {
    if ts.seconds < 0 || ts.nanos < 0 {
        return None;
    }
    let secs = ts.seconds as u128;
    Some(secs * 1_000_000_000 + ts.nanos as u128)
}

#[cfg(test)]
mod tests {
    use super::*;
    use yellowstone_grpc_proto::prelude::{
        SubscribeUpdateAccount, SubscribeUpdateAccountInfo, SubscribeUpdateSlot,
        SubscribeUpdateTransaction, SubscribeUpdateTransactionInfo,
    };
    use yellowstone_grpc_proto::solana::storage::confirmed_block::{Message, Transaction};

    fn account_update(pubkey: [u8; 32], data: Vec<u8>, write_version: u64) -> SubscribeUpdate {
        SubscribeUpdate {
            update_oneof: Some(UpdateOneof::Account(SubscribeUpdateAccount {
                account: Some(SubscribeUpdateAccountInfo {
                    pubkey: pubkey.to_vec(),
                    lamports: 42,
                    owner: [9u8; 32].to_vec(),
                    data,
                    write_version,
                    txn_signature: Some(vec![1u8; 64]),
                    ..Default::default()
                }),
                slot: 100,
                is_startup: false,
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    #[test]
    fn normalization_is_deterministic() {
        let update = account_update([1u8; 32], vec![1, 2, 3], 7);
        let a = match normalize(&update, 111) {
            Normalized::Event(e) => e,
            other => panic!("expected event, got {other:?}"),
        };
        let b = match normalize(&update, 111) {
            Normalized::Event(e) => e,
            other => panic!("expected event, got {other:?}"),
        };
        assert_eq!(a.kind, b.kind);
        assert_eq!(a.arrival_ns, b.arrival_ns);
    }

    #[test]
    fn slot_status_maps() {
        let update = SubscribeUpdate {
            update_oneof: Some(UpdateOneof::Slot(SubscribeUpdateSlot {
                slot: 5,
                parent: Some(4),
                status: yellowstone_grpc_proto::prelude::SlotStatus::SlotConfirmed as i32,
                ..Default::default()
            })),
            ..Default::default()
        };
        match normalize(&update, 1) {
            Normalized::Event(e) => match e.kind {
                EventKind::Slot(s) => {
                    assert_eq!(s.slot, 5);
                    assert_eq!(s.parent, Some(4));
                    assert_eq!(s.status, SlotStatusKind::Confirmed);
                }
                other => panic!("expected slot, got {other:?}"),
            },
            other => panic!("expected event, got {other:?}"),
        }
    }

    #[test]
    fn transaction_keys_are_bounded_and_deduped() {
        let keys: Vec<Vec<u8>> = (0..200u16).map(|i| vec![i as u8; 32]).collect();
        let update = SubscribeUpdate {
            update_oneof: Some(UpdateOneof::Transaction(SubscribeUpdateTransaction {
                transaction: Some(SubscribeUpdateTransactionInfo {
                    signature: vec![2u8; 64],
                    transaction: Some(Transaction {
                        signatures: vec![vec![2u8; 64]],
                        message: Some(Message {
                            account_keys: keys,
                            ..Default::default()
                        }),
                    }),
                    meta: None,
                    index: 1,
                    ..Default::default()
                }),
                slot: 3,
                ..Default::default()
            })),
            ..Default::default()
        };
        match normalize(&update, 1) {
            Normalized::Event(e) => match e.kind {
                EventKind::Transaction(t) => {
                    assert!(t.keys.len() <= MAX_TX_KEYS);
                    let mut sorted = t.keys.clone();
                    sorted.sort_unstable();
                    sorted.dedup();
                    assert_eq!(sorted, t.keys, "keys must be sorted and deduped");
                }
                other => panic!("expected tx, got {other:?}"),
            },
            other => panic!("expected event, got {other:?}"),
        }
    }

    #[test]
    fn ping_is_keepalive() {
        let update = SubscribeUpdate {
            update_oneof: Some(UpdateOneof::Ping(Default::default())),
            ..Default::default()
        };
        assert!(matches!(normalize(&update, 1), Normalized::Keepalive));
    }
}
