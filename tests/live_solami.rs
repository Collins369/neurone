//! Live integration tests against Solami Yellowstone.
//!
//! These are intentionally **separate from the deterministic suite** and are
//! `#[ignore]`d by default: they require a real credential in the environment
//! (`SOLAMI_GRPC_TOKEN` / `SOLAMI_API_KEY`). Run explicitly with:
//!
//! ```text
//! SOLAMI_GRPC_TOKEN=... cargo test --test live_solami -- --ignored --nocapture
//! ```

use std::sync::Arc;
use std::time::Duration;

use neurone::config::{Config, IngestSource};
use neurone::engine::Engine;
use neurone::ingest::solami::{self, TonicConnector};
use neurone::quote::{quote, Side};
use neurone::shutdown::shutdown_channel;
use neurone::telemetry::Metrics;

fn token_present() -> bool {
    std::env::var("SOLAMI_GRPC_TOKEN")
        .or_else(|_| std::env::var("SOLAMI_GRPC_API_KEY"))
        .or_else(|_| std::env::var("SOLAMI_API_KEY"))
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a live Solami credential"]
async fn live_authenticated_stream_decodes_markets() {
    let _ = dotenvy::dotenv();
    if !token_present() {
        eprintln!("skipping: no Solami credential in environment");
        return;
    }

    let mut config = Config::default();
    config.ingest.source = IngestSource::Solami;
    // Keep the stream narrow and bounded.
    config.ingest.filters.account_programs.clear();
    config.ingest.filters.account_addresses.clear();

    let metrics = Metrics::new(
        config.runtime.shards,
        config.telemetry.latency_boundaries_ns.clone(),
    );
    let (handle, shutdown) = shutdown_channel();
    let (engine, shard_handles) = Engine::start(
        config.runtime.shards,
        config.runtime.shard_channel_capacity,
        Arc::clone(&metrics),
        shutdown.clone(),
    );
    let token = solami::load_token().expect("token");
    let connector = TonicConnector::new(&config.ingest, token);

    // Keep a clone so markets can be inspected while the stream is live.
    let inspector = engine.clone();
    let ingest = solami::run(
        &config.ingest,
        &connector,
        engine,
        Arc::clone(&metrics),
        shutdown,
    );
    tokio::pin!(ingest);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let s = metrics.snapshot();
        if s.events_received > 0 && s.active_market_states > 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no live events/markets within 30s: {s:?}"
        );
        tokio::select! {
            r = &mut ingest => panic!("ingestion ended early: {r:?}"),
            _ = tokio::time::sleep(Duration::from_millis(200)) => {}
        }
    }
    // Live reserve state + exact quote on a real market (before shutdown, while
    // the shards are still running).
    let snapshots = inspector.snapshot().await.expect("snapshot");
    let with_reserves = snapshots
        .iter()
        .flat_map(|s| s.markets.iter())
        .filter(|m| m.reserves_known)
        .count();
    if let Some(m) = snapshots
        .iter()
        .flat_map(|s| s.markets.iter())
        .find(|m| m.reserves_known && m.venue == Some(neurone::decode::Venue::PumpSwap))
    {
        let q = quote(m, Side::Sell, 100_000, 25, m.last_reserve_slot, 150);
        println!("live pump.swap sell quote on {}: {q:?}", m.key);
    }

    handle.trigger();
    let _ = ingest.await;
    for h in shard_handles {
        let _ = h.await;
    }

    let s = metrics.snapshot();
    println!("live snapshot: {s:?}");
    assert!(s.events_received > 0);
    assert!(s.events_decoded > 0, "expected protocol decoding");
    assert!(s.active_market_states > 0, "expected markets");
    assert!(
        with_reserves > 0,
        "expected at least one market with known reserves"
    );
}
