//! Live Solami Yellowstone gRPC ingestion.
//!
//! Interface used (see `docs/SOLAMI_RESEARCH.md` for the source of each fact):
//! * endpoint: `https://grpc.solami.dev:443` (TLS, `x-token` metadata);
//! * client: the canonical Yellowstone gRPC client
//!   (`yellowstone-grpc-client`), which speaks the `geyser.Geyser/Subscribe`
//!   bidi stream;
//! * subscription: a normal Yellowstone `SubscribeRequest` (accounts / slots /
//!   transactions / blocks-meta filters + commitment);
//! * keepalive: a periodic `SubscribeRequest` carrying a `ping`;
//! * recovery: reconnect with `from_slot = last_seen + 1`.
//!
//! The transport sits behind the [`Connector`] trait so the reconnect state
//! machine can be exercised with an in-memory stream in tests. Production uses
//! [`TonicConnector`].

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures::{Sink, SinkExt, Stream, StreamExt};
use tokio::time::Instant;
use yellowstone_grpc_client::{ClientTlsConfig, GeyserGrpcClient};
use yellowstone_grpc_proto::prelude::{
    SubscribeRequest, SubscribeRequestFilterAccounts, SubscribeRequestFilterAccountsFilter,
    SubscribeRequestFilterBlocksMeta, SubscribeRequestFilterSlots,
    SubscribeRequestFilterTransactions, SubscribeRequestPing, SubscribeUpdate,
};
use yellowstone_grpc_proto::tonic::Status;

use crate::config::IngestConfig;
use crate::engine::Engine;
use crate::error::{Error, Result};
use crate::events::{normalize, Normalized};
use crate::shutdown::Shutdown;
use crate::telemetry::Metrics;

/// A connectable Yellowstone transport.
///
/// `subscribe` performs one connection attempt and installs `request`. A fresh
/// connection per attempt keeps reconnect semantics simple and mirrors the
/// client's own usage.
pub trait Connector: Send + Sync + 'static {
    type Sink: Sink<SubscribeRequest> + Unpin + Send + 'static;
    type Stream: Stream<Item = std::result::Result<SubscribeUpdate, Status>>
        + Unpin
        + Send
        + 'static;

    fn subscribe(
        &self,
        request: SubscribeRequest,
    ) -> impl Future<Output = Result<(Self::Sink, Self::Stream)>> + Send;
}

/// How a single stream session ended.
enum StreamLoop {
    Shutdown,
    Ended,
}

/// Connect, stream, reconnect with backoff until shutdown.
pub async fn run<C: Connector>(
    cfg: &IngestConfig,
    connector: &C,
    engine: Engine,
    metrics: Arc<Metrics>,
    shutdown: Shutdown,
) -> Result<()>
where
    <C::Sink as Sink<SubscribeRequest>>::Error: std::fmt::Debug,
{
    tracing::info!(endpoint = %cfg.endpoint, commitment = ?cfg.commitment, "starting Solami Yellowstone ingestion");

    let mut last_slot: Option<u64> = None;
    let mut backoff = cfg.reconnect_initial_ms;
    let mut ping_id: i32 = 0;

    loop {
        if shutdown.is_cancelled() {
            break;
        }

        let from_slot = if cfg.replay_from_last_slot {
            last_slot.map(|s| s.saturating_add(1))
        } else {
            None
        };
        let request = build_subscribe_request(cfg, from_slot)?;

        match connector.subscribe(request).await {
            Ok((mut sink, mut stream)) => {
                metrics.set_connected(true);
                backoff = cfg.reconnect_initial_ms;
                tracing::info!(from_slot = ?from_slot, "connected to Solami Yellowstone");
                match run_stream_loop(
                    &mut stream,
                    &mut sink,
                    cfg,
                    &engine,
                    &metrics,
                    &shutdown,
                    &mut last_slot,
                    &mut ping_id,
                )
                .await
                {
                    StreamLoop::Shutdown => break,
                    StreamLoop::Ended => {}
                }
                metrics.set_connected(false);
            }
            Err(e) => {
                metrics.incr_errors();
                tracing::warn!(error = %e, "Solami connect/subscribe failed");
                // A replay request can fail if the gap exceeds Solami's
                // ~3,500-slot replay window. Drop the watermark so the next
                // attempt resyncs live instead of repeating a doomed replay.
                if from_slot.is_some() {
                    tracing::warn!("dropping slot watermark and retrying from live");
                    last_slot = None;
                }
            }
        }

        if shutdown.is_cancelled() {
            break;
        }
        metrics.incr_reconnects();
        tracing::warn!(backoff_ms = backoff, "reconnecting to Solami Yellowstone");
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = tokio::time::sleep(Duration::from_millis(backoff)) => {}
        }
        // Exponential backoff, capped.
        backoff = backoff.saturating_mul(2).min(cfg.reconnect_max_ms.max(1));
    }
    Ok(())
}

/// Drive one live stream until it ends, errors, or shutdown.
#[allow(clippy::too_many_arguments)] // explicit hot-path state, no hidden struct
async fn run_stream_loop<S, K>(
    stream: &mut S,
    sink: &mut K,
    cfg: &IngestConfig,
    engine: &Engine,
    metrics: &Arc<Metrics>,
    shutdown: &Shutdown,
    last_slot: &mut Option<u64>,
    ping_id: &mut i32,
) -> StreamLoop
where
    S: Stream<Item = std::result::Result<SubscribeUpdate, Status>> + Unpin,
    K: Sink<SubscribeRequest> + Unpin,
    K::Error: std::fmt::Debug,
{
    let stale_timeout = Duration::from_millis(cfg.stale_stream_timeout_ms);
    let mut ping_timer = tokio::time::interval(Duration::from_millis(cfg.ping_interval_ms));
    ping_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping_timer.tick().await; // consume the immediate first tick

    let stale = tokio::time::sleep(stale_timeout);
    tokio::pin!(stale);

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return StreamLoop::Shutdown,
            _ = ping_timer.tick() => {
                *ping_id = ping_id.wrapping_add(1);
                let ping = SubscribeRequest {
                    ping: Some(SubscribeRequestPing { id: *ping_id }),
                    ..Default::default()
                };
                if let Err(e) = sink.send(ping).await {
                    tracing::warn!(error = ?e, "failed to send keepalive ping");
                    return StreamLoop::Ended;
                }
            }
            _ = &mut stale => {
                tracing::warn!(stale_ms = cfg.stale_stream_timeout_ms, "stream considered stale; forcing reconnect");
                return StreamLoop::Ended;
            }
            item = stream.next() => {
                match item {
                    Some(Ok(update)) => {
                        stale.as_mut().reset(Instant::now() + stale_timeout);
                        handle_update(update, engine, metrics, last_slot).await;
                    }
                    Some(Err(status)) => {
                        tracing::warn!(status = %status, "Solami stream error");
                        return StreamLoop::Ended;
                    }
                    None => {
                        tracing::warn!("Solami stream closed by server");
                        return StreamLoop::Ended;
                    }
                }
            }
        }
    }
}

async fn handle_update(
    update: SubscribeUpdate,
    engine: &Engine,
    metrics: &Arc<Metrics>,
    last_slot: &mut Option<u64>,
) {
    metrics.incr_received();
    let arrival_ns = crate::clock::now_ns();
    if let Some(slot) = update_slot(&update) {
        *last_slot = Some(last_slot.map_or(slot, |s| s.max(slot)));
        metrics.observe_latest_slot(slot);
    }
    if matches!(
        update.update_oneof.as_ref(),
        Some(yellowstone_grpc_proto::prelude::subscribe_update::UpdateOneof::Slot(_))
    ) {
        metrics.observe_slot();
    }
    let decode_start = crate::clock::now_ns();
    let normalized = normalize(&update, arrival_ns);
    metrics.record_decode(
        (crate::clock::now_ns().saturating_sub(decode_start)).min(u64::MAX as u128) as u64,
    );
    match normalized {
        Normalized::Event(event) => {
            metrics.incr_normalized();
            if let Err(e) = engine.route(event).await {
                metrics.incr_errors();
                tracing::error!(error = %e, "failed to route event; shards may be shutting down");
            }
        }
        Normalized::Keepalive => {}
        Normalized::Ignored => metrics.incr_invalid(),
    }
}

fn update_slot(update: &SubscribeUpdate) -> Option<u64> {
    use yellowstone_grpc_proto::prelude::subscribe_update::UpdateOneof;
    match update.update_oneof.as_ref()? {
        UpdateOneof::Slot(s) => Some(s.slot),
        UpdateOneof::Account(a) => Some(a.slot),
        UpdateOneof::Transaction(t) => Some(t.slot),
        UpdateOneof::BlockMeta(b) => Some(b.slot),
        UpdateOneof::TransactionStatus(s) => Some(s.slot),
        _ => None,
    }
}

/// Load the Solami gRPC credential from the environment (never a file).
pub fn load_token() -> Result<String> {
    std::env::var("SOLAMI_GRPC_TOKEN")
        .or_else(|_| std::env::var("SOLAMI_GRPC_API_KEY"))
        .or_else(|_| std::env::var("SOLAMI_API_KEY"))
        .ok()
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| {
            Error::Config(
                "SOLAMI_GRPC_TOKEN (or SOLAMI_API_KEY) must be set for source=solami".into(),
            )
        })
}

/// Production transport: TLS + `x-token` against a Solami endpoint.
pub struct TonicConnector {
    endpoint: String,
    token: String,
    connect_timeout: Duration,
}

impl TonicConnector {
    pub fn new(cfg: &IngestConfig, token: String) -> Self {
        Self {
            endpoint: cfg.endpoint.clone(),
            token,
            connect_timeout: Duration::from_millis(cfg.connect_timeout_ms),
        }
    }
}

impl Connector for TonicConnector {
    type Sink = yellowstone_grpc_client::SubscribeRequestSink;
    type Stream = yellowstone_grpc_client::GeyserStream;

    fn subscribe(
        &self,
        request: SubscribeRequest,
    ) -> impl Future<Output = Result<(Self::Sink, Self::Stream)>> + Send {
        let endpoint = self.endpoint.clone();
        let token = self.token.clone();
        let connect_timeout = self.connect_timeout;
        async move {
            let mut client = build_client(&endpoint, &token, connect_timeout).await?;
            client
                .subscribe_with_request(Some(request))
                .await
                .map_err(|e| Error::Ingest(format!("subscribe failed: {e}")))
        }
    }
}

async fn build_client(
    endpoint: &str,
    token: &str,
    connect_timeout: Duration,
) -> Result<GeyserGrpcClient> {
    install_crypto_provider();
    let builder = GeyserGrpcClient::build_from_shared(endpoint.to_string())
        .map_err(|e| Error::Ingest(format!("invalid endpoint {endpoint}: {e}")))?;
    builder
        .x_token(Some(token.to_string()))
        .map_err(|e| Error::Ingest(format!("invalid x-token: {e}")))?
        .tls_config(ClientTlsConfig::new().with_native_roots())
        .map_err(|e| Error::Ingest(format!("TLS configuration failed: {e}")))?
        .connect_timeout(connect_timeout)
        .http2_keep_alive_interval(Duration::from_secs(20))
        .connect()
        .await
        .map_err(|e| Error::Ingest(format!("connect failed: {e}")))
}

/// The dependency graph enables more than one rustls crypto backend, so the
/// process-level provider must be chosen explicitly. Ring is installed once;
/// repeat calls are ignored.
fn install_crypto_provider() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// Build the Yellowstone `SubscribeRequest` from configuration.
///
/// Public so tests (and `neurone check`) can assert the exact filter shape.
/// An offset-0 memcmp discriminator filter (base58), used to narrow an account
/// subscription to a single account type.
fn memcmp_disc_filter(base58: &str) -> SubscribeRequestFilterAccountsFilter {
    use yellowstone_grpc_proto::prelude::subscribe_request_filter_accounts_filter::Filter;
    use yellowstone_grpc_proto::prelude::subscribe_request_filter_accounts_filter_memcmp::Data;
    SubscribeRequestFilterAccountsFilter {
        filter: Some(Filter::Memcmp(
            yellowstone_grpc_proto::prelude::SubscribeRequestFilterAccountsFilterMemcmp {
                offset: 0,
                data: Some(Data::Base58(base58.to_string())),
            },
        )),
    }
}

pub fn build_subscribe_request(
    cfg: &IngestConfig,
    from_slot: Option<u64>,
) -> Result<SubscribeRequest> {
    let mut accounts: HashMap<String, SubscribeRequestFilterAccounts> = HashMap::new();
    if !cfg.filters.account_programs.is_empty() || !cfg.filters.account_addresses.is_empty() {
        // Optional anchor discriminator filter (offset 0) to stream a single
        // pump.fun account type instead of every account owned by the program.
        let mut filters = Vec::new();
        if let Some(disc) = &cfg.filters.account_memcmp_base58 {
            filters.push(memcmp_disc_filter(disc));
        }
        accounts.insert(
            "accounts".to_string(),
            SubscribeRequestFilterAccounts {
                account: cfg.filters.account_addresses.clone(),
                owner: cfg.filters.account_programs.clone(),
                filters,
                nonempty_txn_signature: None,
                ..Default::default()
            },
        );
    }
    // Additional bounded account subscriptions (one account type each, e.g.
    // PumpSwap `Pool` accounts). Never a token-account firehose.
    for (i, extra) in cfg.filters.extra_accounts.iter().enumerate() {
        if extra.programs.is_empty() {
            continue;
        }
        let mut filters = Vec::new();
        if let Some(disc) = &extra.memcmp_base58 {
            filters.push(memcmp_disc_filter(disc));
        }
        accounts.insert(
            format!("accounts_extra_{i}"),
            SubscribeRequestFilterAccounts {
                account: Vec::new(),
                owner: extra.programs.clone(),
                filters,
                nonempty_txn_signature: None,
                ..Default::default()
            },
        );
    }

    // Exact-address SOL/USD reference accounts (Pyth `PriceUpdateV2`). One
    // bounded address filter; decoded into the reference state, not a market.
    if !cfg.filters.sol_usd_accounts.is_empty() {
        accounts.insert(
            "sol_usd".to_string(),
            SubscribeRequestFilterAccounts {
                account: cfg.filters.sol_usd_accounts.clone(),
                owner: Vec::new(),
                filters: Vec::new(),
                nonempty_txn_signature: None,
                ..Default::default()
            },
        );
    }

    let mut transactions: HashMap<String, SubscribeRequestFilterTransactions> = HashMap::new();
    if !cfg.filters.transaction_programs.is_empty() {
        transactions.insert(
            "txs".to_string(),
            SubscribeRequestFilterTransactions {
                vote: Some(false),
                failed: None,
                signature: None,
                account_include: cfg.filters.transaction_programs.clone(),
                account_exclude: Vec::new(),
                account_required: Vec::new(),
                ..Default::default()
            },
        );
    }

    let mut slots: HashMap<String, SubscribeRequestFilterSlots> = HashMap::new();
    if cfg.filters.slot_updates {
        slots.insert(
            "slots".to_string(),
            SubscribeRequestFilterSlots {
                filter_by_commitment: Some(false),
                interslot_updates: Some(true),
            },
        );
    }

    let mut blocks_meta: HashMap<String, SubscribeRequestFilterBlocksMeta> = HashMap::new();
    if cfg.filters.block_meta {
        blocks_meta.insert(
            "blocks_meta".to_string(),
            SubscribeRequestFilterBlocksMeta {},
        );
    }

    let mut accounts_data_slice = Vec::new();
    if let Some(len) = cfg.filters.account_data_slice_len {
        accounts_data_slice.push(
            yellowstone_grpc_proto::prelude::SubscribeRequestAccountsDataSlice {
                offset: 0,
                length: len,
            },
        );
    }

    Ok(SubscribeRequest {
        accounts,
        slots,
        transactions,
        transactions_status: HashMap::new(),
        blocks: HashMap::new(),
        blocks_meta,
        entry: HashMap::new(),
        commitment: Some(cfg.commitment.to_proto()),
        accounts_data_slice,
        ping: None,
        from_slot,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Commitment;

    #[test]
    fn request_includes_scoped_filters() {
        let c = IngestConfig {
            commitment: Commitment::Processed,
            ..Default::default()
        };
        let req = build_subscribe_request(&c, Some(42)).unwrap();
        assert_eq!(req.from_slot, Some(42));
        assert_eq!(req.commitment, Some(Commitment::Processed.to_proto()));
        let tx = req.transactions.get("txs").expect("tx filter");
        assert!(!tx.account_include.is_empty());
        assert_eq!(tx.vote, Some(false));
        // Default configuration subscribes the pump.fun bonding-curve accounts
        // (owner-scoped + BondingCurve memcmp) so a market invalidated by a fee
        // sweep can be re-established from a fresh authoritative account update.
        let acct = req.accounts.get("accounts").expect("account filter");
        assert!(acct
            .owner
            .contains(&"6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P".to_string()));
        assert_eq!(
            acct.filters.len(),
            1,
            "expected the BondingCurve memcmp filter"
        );
        assert!(req.slots.contains_key("slots"));
        assert!(req.blocks_meta.contains_key("blocks_meta"));
    }

    #[test]
    fn account_filter_is_emitted_when_configured() {
        let c = IngestConfig {
            filters: crate::config::FilterConfig {
                account_programs: vec!["6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P".into()],
                account_addresses: vec!["11134iSgWxi8QLbVgrAe48yzbzcgBrd7ZDJjmSC67Wv".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let req = build_subscribe_request(&c, None).unwrap();
        let acct = req.accounts.get("accounts").expect("account filter");
        assert!(!acct.owner.is_empty());
        assert!(!acct.account.is_empty());
    }

    /// The shipped/default configuration must actually deliver the authoritative
    /// bonding-curve account updates that re-establish an invalidated market.
    #[test]
    fn default_subscription_targets_bonding_curves() {
        use yellowstone_grpc_proto::prelude::subscribe_request_filter_accounts_filter::Filter;
        use yellowstone_grpc_proto::prelude::subscribe_request_filter_accounts_filter_memcmp::Data;

        let c = IngestConfig::default();
        let req = build_subscribe_request(&c, None).unwrap();
        let acct = req
            .accounts
            .get("accounts")
            .expect("default account filter");
        assert!(acct
            .owner
            .contains(&"6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P".to_string()));
        match acct.filters.first().and_then(|f| f.filter.as_ref()) {
            Some(Filter::Memcmp(m)) => {
                assert_eq!(m.offset, 0);
                assert_eq!(m.data, Some(Data::Base58("4y6pru6YvC7".to_string())));
            }
            other => panic!("expected BondingCurve memcmp filter, got {other:?}"),
        }
    }

    /// The default configuration also subscribes the PumpSwap `Pool` accounts
    /// (owner = pump_amm + Pool discriminator) so a late pool can bootstrap its
    /// vault balances from transaction `postTokenBalances`. It stays bounded:
    /// two account filters (curves + pools), never a token-account firehose.
    #[test]
    fn default_subscription_targets_pump_swap_pools() {
        use yellowstone_grpc_proto::prelude::subscribe_request_filter_accounts_filter::Filter;
        use yellowstone_grpc_proto::prelude::subscribe_request_filter_accounts_filter_memcmp::Data;

        let c = IngestConfig::default();
        let req = build_subscribe_request(&c, None).unwrap();
        assert_eq!(req.accounts.len(), 3, "curves + pools + pyth sol/usd");
        let pyth = req.accounts.get("sol_usd").expect("sol_usd account filter");
        assert_eq!(
            pyth.account,
            vec!["7UVimffxr9ow1uXYxsr4LHAcV58mLzhmwaeKvJ1pjLiE".to_string()]
        );
        assert!(pyth.owner.is_empty(), "exact-address filter, no program");
        let acct = req
            .accounts
            .get("accounts_extra_0")
            .expect("pump_amm Pool account filter");
        assert!(acct
            .owner
            .contains(&"pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA".to_string()));
        match acct.filters.first().and_then(|f| f.filter.as_ref()) {
            Some(Filter::Memcmp(m)) => {
                assert_eq!(m.offset, 0);
                assert_eq!(m.data, Some(Data::Base58("hQrXeCntzbV".to_string())));
            }
            other => panic!("expected Pool memcmp filter, got {other:?}"),
        }
        // No account filter may target a token program (no token firehose).
        for f in req.accounts.values() {
            for owner in &f.owner {
                assert!(
                    !owner.contains("Token"),
                    "unexpected token-program account subscription: {owner}"
                );
            }
        }
    }

    #[test]
    fn from_slot_omitted_when_disabled() {
        let c = IngestConfig {
            replay_from_last_slot: false,
            ..Default::default()
        };
        let req = build_subscribe_request(&c, None).unwrap();
        assert_eq!(req.from_slot, None);
    }
}
