//! Live Yellowstone probe: does Solami deliver the on-chain Pyth SOL/USD
//! `PriceUpdateV2` account, and how fresh/fast is it?
//!
//! Ignored by default (needs a live Solami credential + network):
//!
//! ```text
//! cargo test --test live_pyth --locked -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use futures::StreamExt;
use neurone::config::{Config, IngestSource};
use neurone::events::MarketKey;
use neurone::ingest::solami::{self, Connector, TonicConnector};
use neurone::pyth;
use yellowstone_grpc_proto::prelude::{subscribe_update::UpdateOneof, SubscribeUpdate};

fn token_present() -> bool {
    std::env::var("SOLAMI_GRPC_TOKEN")
        .or_else(|_| std::env::var("SOLAMI_GRPC_API_KEY"))
        .or_else(|_| std::env::var("SOLAMI_API_KEY"))
        .map(|t| !t.trim().is_empty())
        .unwrap_or(false)
}

fn pct(xs: &[u64], q: f64) -> u64 {
    if xs.is_empty() {
        return 0;
    }
    let mut s = xs.to_vec();
    s.sort_unstable();
    s[((s.len() - 1) as f64 * q) as usize]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a live Solami credential"]
async fn live_pyth_sol_usd_account_streams() {
    let _ = dotenvy::dotenv();
    if !token_present() {
        eprintln!("skipping: no Solami credential in environment");
        return;
    }

    let mut config = Config::default();
    config.ingest.source = IngestSource::Solami;
    // Exactly one bounded account subscription: the Pyth SOL/USD price account.
    config.ingest.filters.transaction_programs.clear();
    config.ingest.filters.account_programs.clear();
    config.ingest.filters.account_addresses = vec![pyth::SOL_USD_PRICE_ACCOUNT.to_string()];
    config.ingest.filters.account_memcmp_base58 = None;
    config.ingest.filters.extra_accounts.clear();
    config.ingest.filters.block_meta = false;

    let request = solami::build_subscribe_request(&config.ingest, None).expect("request");
    let token = solami::load_token().expect("token");
    let connector = TonicConnector::new(&config.ingest, token);

    let target = MarketKey::from_base58(pyth::SOL_USD_PRICE_ACCOUNT).expect("pubkey");
    let start = Instant::now();
    let (_sink, mut stream) = connector.subscribe(request).await.expect("subscribe");

    let seconds = std::env::var("NEURONE_PYTH_SECONDS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(60);
    let deadline = start + Duration::from_secs(seconds);

    let mut first_update: Option<Duration> = None;
    let mut received: Vec<(u64, i64, u64)> = Vec::new(); // (slot, publish_time, price_micros)
    let mut decode_failures = 0u64;
    let mut non_target = 0u64;
    let mut last_slot = 0u64;

    loop {
        let now_remaining = deadline.saturating_duration_since(Instant::now());
        if now_remaining.is_zero() {
            break;
        }
        let Some(item) = tokio::time::timeout(now_remaining, stream.next())
            .await
            .ok()
            .flatten()
        else {
            break;
        };
        let Ok(update) = item else { continue };
        let SubscribeUpdate {
            update_oneof: Some(UpdateOneof::Account(a)),
            ..
        } = update
        else {
            continue;
        };
        let Some(info) = a.account.as_ref() else {
            continue;
        };
        if MarketKey::from_slice(&info.pubkey) != Some(target) {
            non_target += 1;
            continue;
        }
        last_slot = last_slot.max(a.slot);
        match pyth::decode_sol_usd_price_update(&info.data) {
            Some(u) => {
                first_update.get_or_insert_with(|| start.elapsed());
                received.push((a.slot, u.publish_time, u.price_micros().unwrap_or(0)));
            }
            None => decode_failures += 1,
        }
    }

    let n = received.len();
    let now = (neurone::clock::now_ns() / 1_000_000_000) as u64;
    let mut gaps = Vec::new();
    for w in received.windows(2) {
        if w[1].0 >= w[0].0 {
            gaps.push(w[1].0 - w[0].0);
        }
    }
    let ages: Vec<u64> = received
        .iter()
        .map(|(_, pt, _)| now.saturating_sub((*pt).max(0) as u64))
        .collect();
    let prices: Vec<u64> = received.iter().map(|(_, _, p)| *p).collect();

    println!(
        "pyth probe: account={} seconds={seconds}",
        pyth::SOL_USD_PRICE_ACCOUNT
    );
    println!("  updates={n} decode_failures={decode_failures} non_target={non_target}");
    println!("  first_update_after={:?}", first_update);
    println!("  last_account_slot={last_slot}");
    if n > 0 {
        println!(
            "  slot gap: min={} p50={} p95={} max={}",
            gaps.iter().copied().min().unwrap_or(0),
            pct(&gaps, 0.50),
            pct(&gaps, 0.95),
            gaps.iter().copied().max().unwrap_or(0)
        );
        println!(
            "  publish_time age (s): min={} p50={} max={}",
            ages.iter().copied().min().unwrap_or(0),
            pct(&ages, 0.50),
            ages.iter().copied().max().unwrap_or(0)
        );
        println!(
            "  price_usd micros (x1e-6): min={} p50={} max={}",
            prices.iter().copied().min().unwrap_or(0),
            pct(&prices, 0.50),
            prices.iter().copied().max().unwrap_or(0)
        );
        let (_, first_pt, _) = received[0];
        let (_, last_pt, _) = received[n - 1];
        println!("  first publish_time={first_pt} last publish_time={last_pt}");
    }
    // The sponsored feed publishes on a ~55-60 s heartbeat, so short windows can
    // legitimately observe zero updates; only require one for longer windows.
    assert!(
        n > 0 || seconds < 120,
        "no Pyth SOL/USD updates in {seconds}s (heartbeat is ~55-60s; use >=120s)"
    );
    if n == 0 {
        eprintln!("note: no update in {seconds}s < heartbeat; rerun with a longer window");
    }
}
