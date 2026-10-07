//! Ingestion recovery tests.
//!
//! The Solami transport is mocked *only at the connection boundary*: the
//! reconnect loop, normalization, routing and telemetry under test are the real
//! production code paths. Evidence level: integration (transport mocked).

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use neurone::config::{Config, IngestConfig};
use neurone::engine::Engine;
use neurone::ingest::solami::{self, Connector};
use neurone::shutdown::shutdown_channel;
use neurone::telemetry::Metrics;
use yellowstone_grpc_client::{GeyserStream, SubscribeRequestSink};
use yellowstone_grpc_proto::prelude::{
    subscribe_update::UpdateOneof, SubscribeRequest, SubscribeUpdate, SubscribeUpdateAccount,
    SubscribeUpdateAccountInfo, SubscribeUpdateSlot,
};

/// Mock transport: the first `fail_attempts` connections fail, then every
/// connection yields two events and closes (simulating a stream interruption).
struct MockConnector {
    attempts: AtomicU64,
    fail_attempts: u64,
    // Keeps the request-sink receivers alive for the lifetime of the test.
    _sink_receivers: Mutex<Vec<futures::channel::mpsc::Receiver<SubscribeRequest>>>,
}

impl MockConnector {
    fn new(fail_attempts: u64) -> Self {
        Self {
            attempts: AtomicU64::new(0),
            fail_attempts,
            _sink_receivers: Mutex::new(Vec::new()),
        }
    }
}

impl Connector for MockConnector {
    type Sink = SubscribeRequestSink;
    type Stream = GeyserStream;

    fn subscribe(
        &self,
        _request: SubscribeRequest,
    ) -> impl Future<Output = neurone::Result<(Self::Sink, Self::Stream)>> + Send {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
        let fail_attempts = self.fail_attempts;

        let (sink_tx, sink_rx) = futures::channel::mpsc::channel(16);
        self._sink_receivers.lock().unwrap().push(sink_rx);

        let (tx, rx) = tokio::sync::mpsc::channel(16);
        // Two events, then drop the sender so the stream ends.
        let _ = tx.try_send(Ok(account_event(attempt)));
        let _ = tx.try_send(Ok(slot_event(attempt)));
        drop(tx);

        let sink = SubscribeRequestSink::mock(sink_tx);
        let stream = GeyserStream::mock(rx);

        async move {
            if attempt < fail_attempts {
                return Err(neurone::Error::Ingest(format!(
                    "simulated connect failure #{attempt}"
                )));
            }
            Ok((sink, stream))
        }
    }
}

fn account_event(attempt: u64) -> SubscribeUpdate {
    let mut pubkey = [0u8; 32];
    pubkey[..8].copy_from_slice(&attempt.to_le_bytes());
    pubkey[8] = 0x11;
    SubscribeUpdate {
        update_oneof: Some(UpdateOneof::Account(SubscribeUpdateAccount {
            account: Some(SubscribeUpdateAccountInfo {
                pubkey: pubkey.to_vec(),
                lamports: 1,
                owner: vec![7u8; 32],
                data: vec![1, 2, 3],
                write_version: 1,
                ..Default::default()
            }),
            slot: 1_000 + attempt,
            is_startup: false,
            ..Default::default()
        })),
        ..Default::default()
    }
}

fn slot_event(attempt: u64) -> SubscribeUpdate {
    SubscribeUpdate {
        update_oneof: Some(UpdateOneof::Slot(SubscribeUpdateSlot {
            slot: 1_000 + attempt,
            parent: Some(999 + attempt),
            status: 0,
            ..Default::default()
        })),
        ..Default::default()
    }
}

fn fast_config() -> Config {
    let mut cfg = Config::default();
    cfg.ingest = IngestConfig {
        reconnect_initial_ms: 5,
        reconnect_max_ms: 20,
        ping_interval_ms: 1_000,
        stale_stream_timeout_ms: 5_000,
        ..cfg.ingest
    };
    cfg
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ingestion_recovers_from_connect_failures_and_stream_interruptions() {
    let cfg = fast_config();
    let metrics = Metrics::new(
        cfg.runtime.shards,
        cfg.telemetry.latency_boundaries_ns.clone(),
    );
    let (handle, shutdown) = shutdown_channel();
    let (engine, shard_handles) = Engine::start(
        cfg.runtime.shards,
        cfg.runtime.shard_channel_capacity,
        Arc::clone(&metrics),
        shutdown.clone(),
    );
    // Two connect failures, then streams that keep ending.
    let connector = MockConnector::new(2);

    let outcome = tokio::time::timeout(Duration::from_secs(10), async {
        let ingest = solami::run(
            &cfg.ingest,
            &connector,
            engine,
            Arc::clone(&metrics),
            shutdown,
        );
        tokio::pin!(ingest);
        loop {
            let s = metrics.snapshot();
            if s.reconnects >= 4 && s.events_normalized >= 4 {
                break;
            }
            tokio::select! {
                result = &mut ingest => {
                    result.expect("ingestion returned early");
                }
                _ = tokio::time::sleep(Duration::from_millis(5)) => {}
            }
        }
        handle.trigger();
        ingest.await.expect("ingestion run failed");
    })
    .await;

    assert!(
        outcome.is_ok(),
        "ingestion did not recover in time: {outcome:?}"
    );
    for h in shard_handles {
        h.await.expect("shard task panicked");
    }

    let s = metrics.snapshot();
    assert!(s.reconnects >= 4, "expected reconnects, got {s:?}");
    assert!(
        s.events_normalized >= 4,
        "expected normalized events, got {s:?}"
    );
    assert!(
        s.state_updates >= 2,
        "expected market state updates, got {s:?}"
    );
    assert!(
        s.slots_observed >= 1,
        "expected slot updates observed, got {s:?}"
    );
    assert!(!s.yellowstone_connected);
}
