//! Live Yellowstone protocol-parity validation harness (M3).
//!
//! Runs the **real** Solami Yellowstone client + production decoder + reconciled
//! quote formulas, and compares each predicted result against the on-chain
//! result recorded in the same event. Reuses [`crate::ingest::solami`] and
//! [`crate::quote`]; it does not route into shards (this is parity validation,
//! not the trading path).

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;

use crate::config::Config;
use crate::decode::{DecodedSwap, Venue};
use crate::error::Result;
use crate::events::{normalize, Normalized};
use crate::events::{AccountUpdate, EventKind, MarketKey};
use crate::ingest::solami::{self, Connector, TonicConnector};
use crate::quote::{self, Instruction};

/// pump.fun `BondingCurve` anchor discriminator (base58), used as the account
/// subscription's memcmp filter so only bonding curves are streamed.
pub const PUMPFUN_BONDING_CURVE_DISC_B58: &str = "4y6pru6YvC7";
pub const PUMPFUN_PROGRAM_ID: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";

/// Recent bonding-curve states per curve, keyed by the curve account pubkey.
///
/// A small ring keeps enough history to pick the state that is strictly older
/// than a trade's slot, which is the deterministic trade-time pre-state. This
/// avoids racing a newer (post-trade) account update against an older trade.
const CURVE_HISTORY: usize = 4;

#[derive(Default)]
struct CurveHistory {
    /// (slot, virtual_base, virtual_quote) newest-last.
    states: VecDeque<(u64, u128, i128)>,
    layout_len: usize,
}

impl CurveHistory {
    fn push(&mut self, slot: u64, vb: u128, vq: i128, layout_len: usize) {
        self.layout_len = layout_len;
        if let Some(last) = self.states.back_mut() {
            if last.0 == slot {
                *last = (slot, vb, vq);
                return;
            }
            if slot < last.0 {
                // Out-of-order: keep the ring sorted by slot.
                if let Some(pos) = self.states.iter().position(|s| s.0 >= slot) {
                    if self.states[pos].0 == slot {
                        self.states[pos] = (slot, vb, vq);
                    } else {
                        self.states.insert(pos, (slot, vb, vq));
                    }
                } else {
                    self.states.push_back((slot, vb, vq));
                }
            } else {
                self.states.push_back((slot, vb, vq));
            }
        } else {
            self.states.push_back((slot, vb, vq));
        }
        while self.states.len() > CURVE_HISTORY {
            self.states.pop_front();
        }
    }

    /// Newest state strictly older than `trade_slot`.
    fn pre_state(&self, trade_slot: u64) -> Option<(u128, i128)> {
        self.states
            .iter()
            .rev()
            .find(|(slot, _, _)| *slot < trade_slot)
            .map(|(_, vb, vq)| (*vb, *vq))
    }
}

/// Deterministic in-memory bonding-curve state cache.
#[derive(Default)]
pub struct CurveCache {
    curves: HashMap<MarketKey, CurveHistory>,
    pub updates: u64,
    pub startup_updates: u64,
}

impl CurveCache {
    fn record(&mut self, a: &AccountUpdate) {
        let Some(decoded) = &a.decoded else { return };
        if decoded.venue != Venue::PumpFun {
            return;
        }
        let (Some(vb), Some(vq)) = (decoded.virtual_base_reserve, decoded.virtual_quote_reserve)
        else {
            return;
        };
        self.updates += 1;
        if a.is_startup {
            self.startup_updates += 1;
        }
        self.curves
            .entry(a.pubkey)
            .or_default()
            .push(a.slot, vb, vq, a.data_len as usize);
    }

    /// Number of curves currently cached.
    pub fn len(&self) -> usize {
        self.curves.len()
    }

    pub fn is_empty(&self) -> bool {
        self.curves.is_empty()
    }
}

/// Per-(venue, instruction) parity counters.
#[derive(Default, Clone, Debug)]
pub struct PathStats {
    pub samples: u64,
    pub exact: u64,
    pub mismatches: u64,
    pub errors: u64,
    pub unsupported_state: u64,
    pub example_mismatch: Option<String>,
    pub example_error: Option<String>,
    pub example_unsupported: Option<String>,
}

/// Live validation outcome.
#[derive(Clone, Debug, Default)]
pub struct ValidationReport {
    pub connected: bool,
    pub updates: u64,
    pub swaps: u64,
    pub decode_ms: Vec<u64>,
    pub quote_ns: Vec<u64>,
    pub paths: BTreeMap<String, PathStats>,
    pub curve_updates: u64,
    pub curve_startup_updates: u64,
    pub curves_cached: usize,
    pub curve_corroborations: u64,
    pub curve_corroborated: u64,
    /// Why the pre-state gate rejected a trade (reason -> count).
    pub unsupported_reasons: BTreeMap<&'static str, u64>,
}

impl ValidationReport {
    fn path(&mut self, key: String) -> &mut PathStats {
        self.paths.entry(key).or_default()
    }

    /// Render the parity matrix as text.
    pub fn matrix(&self) -> String {
        let mut out = String::from(
            "venue/instruction            samples  exact  mismatch  errors  unsupported\n",
        );
        for (k, s) in &self.paths {
            out.push_str(&format!(
                "{:<28} {:>7} {:>6} {:>9} {:>7} {:>10}\n",
                k, s.samples, s.exact, s.mismatches, s.errors, s.unsupported_state
            ));
        }
        out
    }

    pub fn percentiles(&self, mut v: Vec<u64>) -> (u64, u64, u64) {
        if v.is_empty() {
            return (0, 0, 0);
        }
        v.sort_unstable();
        let pick = |q: f64| {
            v[(((v.len() as f64) * q).ceil() as usize)
                .saturating_sub(1)
                .min(v.len() - 1)]
        };
        (pick(0.50), pick(0.95), pick(0.99))
    }
}

/// Validate live swaps for `seconds`.
pub async fn run(config: &Config, seconds: u64) -> Result<ValidationReport> {
    // Enable the pump.fun bonding-curve account stream for the correlation test
    // (narrow: owner = pump.fun program + BondingCurve discriminator).
    let mut config = config.clone();
    if !config
        .ingest
        .filters
        .account_programs
        .iter()
        .any(|p| p == PUMPFUN_PROGRAM_ID)
    {
        config
            .ingest
            .filters
            .account_programs
            .push(PUMPFUN_PROGRAM_ID.to_string());
    }
    config.ingest.filters.account_memcmp_base58 = Some(PUMPFUN_BONDING_CURVE_DISC_B58.to_string());
    let config = &config;

    let token = solami::load_token()?;
    let connector = TonicConnector::new(&config.ingest, token);
    let request = solami::build_subscribe_request(&config.ingest, None)?;
    let (_sink, mut stream) = connector.subscribe(request).await?;

    let mut report = ValidationReport {
        connected: true,
        ..Default::default()
    };
    let mut cache = CurveCache::default();
    // Last observed post-trade state per pump.fun curve (for contiguity).
    let mut last_event: HashMap<MarketKey, (u64, u128, i128)> = HashMap::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);

    loop {
        let update = tokio::select! {
            _ = tokio::time::sleep_until(deadline) => break,
            item = stream.next() => item,
        };
        let Some(Ok(update)) = update else { break };
        report.updates += 1;

        let t0 = crate::clock::now_ns();
        let normalized = normalize(&update, t0);
        let t1 = crate::clock::now_ns();
        let Normalized::Event(event) = normalized else {
            continue;
        };
        report.decode_ms.push(((t1 - t0) / 1_000) as u64);
        if let EventKind::Account(a) = &event.kind {
            cache.record(a);
            continue;
        }
        let EventKind::Transaction(tx) = &event.kind else {
            continue;
        };
        for swap in &tx.swaps {
            report.swaps += 1;
            let key = format!("{}/{}", swap.venue.as_str(), instruction_label(swap));
            // Pre-state corroboration for pump.fun SELL / token-target BUY.
            let mut corroborated = true;
            if swap.venue == Venue::PumpFun {
                if let Ok(s) = quote::swap_pre_state(swap) {
                    report.curve_corroborations += 1;
                    // (a) contiguous previous event on the same curve.
                    let contig = last_event
                        .get(&swap.market_key)
                        .map(|(slot, vb, vq)| {
                            // `<=`: multiple trades can share a slot; the
                            // post-state equality is the real evidence.
                            *slot <= tx.slot && *vb == s.pre_base && (*vq as u128) == s.pre_quote
                        })
                        .unwrap_or(false);
                    // (b) bonding-curve account state strictly older than the trade.
                    let acct = cache
                        .curves
                        .get(&swap.market_key)
                        .and_then(|h| h.pre_state(tx.slot))
                        .map(|(vb, vq)| vb == s.pre_base && (vq as u128) == s.pre_quote)
                        .unwrap_or(false);
                    if contig || acct {
                        report.curve_corroborated += 1;
                    } else if !matches!(quote::classify(swap), Ok(Instruction::BuyExactIn)) {
                        // SELL / token-target require corroborated pre-state.
                        corroborated = false;
                        let reason = if !last_event.contains_key(&swap.market_key) {
                            "no_previous_event_observed"
                        } else if !contig {
                            "previous_event_not_contiguous"
                        } else if !cache.curves.contains_key(&swap.market_key) {
                            "no_account_state_cached"
                        } else if cache
                            .curves
                            .get(&swap.market_key)
                            .and_then(|h| h.pre_state(tx.slot))
                            .is_none()
                        {
                            "account_state_not_older_than_trade"
                        } else {
                            "account_state_mismatch"
                        };
                        *report.unsupported_reasons.entry(reason).or_insert(0) += 1;
                    }
                    // Store the event's POST-trade state (its own stored virtual
                    // reserves), which is the pre-state of the next trade on
                    // this curve.
                    if let (Some(vb), Some(vq)) =
                        (swap.virtual_base_reserve, swap.virtual_quote_reserve)
                    {
                        last_event.insert(swap.market_key, (tx.slot, u128::from(vb), vq));
                    }
                }
            }
            let tq = crate::clock::now_ns();
            let outcome = if corroborated {
                evaluate(swap)
            } else {
                Err(quote::QuoteError::UnsupportedState)
            };
            report
                .quote_ns
                .push((crate::clock::now_ns() - tq).min(u64::MAX as u128) as u64);
            let path = report.path(key);
            match outcome {
                Ok(true) => {
                    path.samples += 1;
                    path.exact += 1;
                }
                Ok(false) => {
                    path.samples += 1;
                    path.mismatches += 1;
                    if path.example_mismatch.is_none() {
                        path.example_mismatch = Some(describe(swap));
                    }
                }
                Err(e) => {
                    if matches!(e, quote::QuoteError::UnsupportedState) {
                        path.unsupported_state += 1;
                        if path.example_unsupported.is_none() {
                            path.example_unsupported = Some(describe(swap));
                        }
                    } else {
                        path.errors += 1;
                        if path.example_error.is_none() {
                            path.example_error = Some(format!("{e:?}: {}", describe(swap)));
                        }
                    }
                }
            }
        }
    }
    report.curve_updates = cache.updates;
    report.curve_startup_updates = cache.startup_updates;
    report.curves_cached = cache.len();
    Ok(report)
}

fn instruction_label(swap: &DecodedSwap) -> String {
    quote::classify(swap)
        .map(|i| i.as_str().to_string())
        .unwrap_or_else(|_| {
            format!(
                "unknown({})",
                swap.ix_name.clone().unwrap_or_else(|| "-".into())
            )
        })
}

/// Evaluate one swap against its own trade-time pre-state.
///
/// Returns `Ok(true)` for an exact integer match against the on-chain result.
/// Coherence guard: the pump.fun `TradeEvent` has grown over time, and a
/// subset of live payloads decodes to values that are impossible on-chain
/// (e.g. `token_amount` above the fixed 1e15 supply, a zero reserve with a
/// non-zero trade). Those are counted as `decoder_inconsistent`, never as
/// formula mismatches.
const PUMPFUN_MAX_SUPPLY: u64 = 1_000_000_000_000_000;

fn coherent(swap: &DecodedSwap) -> bool {
    match swap.venue {
        Venue::PumpFun => {
            swap.quote_amount > 0
                && swap.base_amount > 0
                && swap.base_amount <= PUMPFUN_MAX_SUPPLY
                && swap.virtual_base_reserve.unwrap_or(0) > 0
                && swap.virtual_quote_reserve.unwrap_or(0) > 0
        }
        Venue::PumpSwap => swap.quote_amount > 0 && swap.base_amount > 0,
    }
}

fn evaluate(swap: &DecodedSwap) -> std::result::Result<bool, quote::QuoteError> {
    if !coherent(swap) {
        return Err(quote::QuoteError::ZeroReserves);
    }
    let instruction = quote::classify(swap)?;
    Ok(match instruction {
        Instruction::Sell => quote::predict_swap(swap)? == u128::from(swap.quote_amount),
        Instruction::BuyExactIn => quote::predict_swap(swap)? == u128::from(swap.base_amount),
        Instruction::BuyTokenTarget => {
            // Token-target: the quote charged is the observed pool quote inflow.
            quote::predict_token_target_quote(swap)? == u128::from(swap.quote_amount)
        }
    })
}

fn describe(swap: &DecodedSwap) -> String {
    format!(
        "venue={:?} ix={:?} buy={} base={} quote={} uq={} base_res={:?} quote_res={:?} virt_b={:?} virt_q={:?}",
        swap.venue,
        swap.ix_name,
        swap.is_buy,
        swap.base_amount,
        swap.quote_amount,
        swap.user_quote_amount,
        swap.base_reserve,
        swap.quote_reserve,
        swap.virtual_base_reserve,
        swap.virtual_quote_reserve
    )
}

/// Convenience for the binary: run and render a text report.
pub async fn run_to_text(config: &Config, seconds: u64) -> Result<String> {
    let report = run(config, seconds).await?;
    let (d50, d95, d99) = report.percentiles(report.decode_ms.clone());
    let (q50, q95, q99) = report.percentiles(report.quote_ns.clone());
    let (q50, q95, q99) = (
        q50 as f64 / 1000.0,
        q95 as f64 / 1000.0,
        q99 as f64 / 1000.0,
    );
    let mut out = format!(
        "connected={} updates={} swaps={} curve_updates={} curve_startup={} curves_cached={} acct_corroborations={} acct_corroborated={}\n{}",
        report.connected,
        report.updates,
        report.swaps,
        report.curve_updates,
        report.curve_startup_updates,
        report.curves_cached,
        report.curve_corroborations,
        report.curve_corroborated,
        report.matrix()
    );
    out.push_str(&format!(
        "decode+normalize us p50={d50} p95={d95} p99={d99}\nquote us p50={q50:.3} p95={q95:.3} p99={q99:.3}\n"
    ));
    let reasons: Vec<String> = report
        .unsupported_reasons
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    out.push_str(&format!("unsupported reasons: {}\n", reasons.join(" ")));
    for (k, s) in &report.paths {
        if let Some(ex) = &s.example_mismatch {
            out.push_str(&format!("  example mismatch [{k}]: {ex}\n"));
        }
        if let Some(ex) = &s.example_error {
            out.push_str(&format!("  example error [{k}]: {ex}\n"));
        }
    }
    Ok(out)
}

/// Re-export for callers that want the Arc-based metrics (unused here).
#[allow(dead_code)]
fn _unused(_: Arc<()>) {}
