//! M4 — Strategy V1 deterministic tests.
//!
//! Price convention: `sol_usd_price_micros = 1_000_000_000` ($1000/SOL), so
//! `usd_micros(quote_raw) == quote_raw` and "$X ⇔ X × 1e6 raw quote units".

use std::sync::Arc;

use neurone::decode::{CreatedMarket, DecodedAccount, DecodedSwap, Venue};
use neurone::engine::Engine;
use neurone::events::{AccountUpdate, EventKind, MarketKey, NormalizedEvent, TransactionUpdate};
use neurone::market::{MarketState, MarketStatus, ReserveState};
use neurone::pyth;
use neurone::quote::{self, Side};
use neurone::shutdown::{shutdown_channel, ShutdownHandle};
use neurone::strategy::SolUsdSource;
use neurone::strategy::{self, Decision, RejectReason, StrategyConfig};
use neurone::telemetry::Metrics;

const PRICE: u64 = 1_000_000_000; // $1000 per SOL
const CUR_SLOT: u64 = 10_000;
const STALE: u64 = 150;

fn key(b: u8) -> MarketKey {
    let mut k = [0u8; 32];
    k[0] = b;
    k[31] = 0x5C;
    MarketKey(k)
}

/// USD → raw quote units at the test price.
fn usd(x: u64) -> u128 {
    u128::from(x) * 1_000_000
}

fn cfg() -> StrategyConfig {
    StrategyConfig {
        sol_usd_price_micros: Some(PRICE),
        // Most tests focus on the other gates; freshness is exercised explicitly.
        max_token_age_seconds: None,
        ..Default::default()
    }
}

/// Config with the locked 15-minute freshness gate enabled.
fn cfg_fresh() -> StrategyConfig {
    StrategyConfig {
        max_token_age_seconds: Some(900),
        ..cfg()
    }
}

/// Pyth-over-Yellowstone SOL/USD mode (no static price). Freshness disabled so
/// these tests isolate the reference behaviour.
fn cfg_pyth() -> StrategyConfig {
    StrategyConfig {
        sol_usd_source: SolUsdSource::PythYellowstone,
        sol_usd_price_micros: None,
        max_token_age_seconds: None,
        ..StrategyConfig::default()
    }
}

/// Build a qualifying pump.fun curve, then override individual dimensions.
///
/// `virtual_quote = liquidity_usd/2 × 1e6`; `supply` is chosen so the MCAP
/// equals `mcap_usd`. `virtual_base = 1e15`.
fn curve(liquidity_usd: u64, mcap_usd: u64, volume_usd: u64, created_at_slot: u64) -> MarketState {
    let vb = 1_000_000_000_000_000u128; // 1e15
    let vq = usd(liquidity_usd) / 2; // so liquidity = 2*vq = usd(liquidity_usd)
    let supply = usd(mcap_usd) * vb / vq; // mcap = vq*supply/vb = usd(mcap_usd)
    let mut m = MarketState::new(key(1), 0);
    m.venue = Some(Venue::PumpFun);
    m.base_mint = Some(key(0xA1));
    m.quote_mint = None; // SOL
    m.token_total_supply = supply;
    m.virtual_base_reserve = vb;
    m.virtual_quote_reserve = vq as i128;
    m.base_reserve = 800_000_000_000_000;
    m.quote_reserve = 5_000_000_000;
    m.reserves_known = true;
    m.last_reserve_slot = CUR_SLOT;
    m.last_trade_slot = CUR_SLOT;
    m.last_slot = CUR_SLOT;
    m.last_fee_bps = Some(95);
    m.created_at_slot = created_at_slot;
    m.peak_mcap_quote = usd(mcap_usd);
    m.volume
        .record(CUR_SLOT - 10, true, usd(volume_usd) as u64, 1);
    m
}

fn base_market() -> MarketState {
    curve(2_000, 5_000, 1_000, 900)
}

fn eval(m: &MarketState, c: &StrategyConfig) -> Decision {
    strategy::evaluate(m, CUR_SLOT, STALE, c, c.sol_usd_price_micros)
}

fn reason(m: &MarketState, c: &StrategyConfig) -> RejectReason {
    match eval(m, c) {
        Decision::Rejected(r) => r,
        Decision::Qualified => panic!("expected rejection, got Qualified"),
    }
}

// ---------------------------------------------------------------- baseline ---

#[test]
fn qualifying_market_is_qualified() {
    assert_eq!(eval(&base_market(), &cfg()), Decision::Qualified);
}

#[test]
fn decision_is_deterministic() {
    let m = base_market();
    let c = cfg();
    let a = eval(&m, &c);
    for _ in 0..1000 {
        assert_eq!(eval(&m, &c), a);
    }
}

// --------------------------------------------------------------- liquidity ---

#[test]
fn liquidity_boundary() {
    let c = cfg();
    assert_eq!(
        reason(&curve(1_999, 5_000, 1_000, 900), &c),
        RejectReason::LowLiquidity
    );
    assert_eq!(
        eval(&curve(2_000, 5_000, 1_000, 900), &c),
        Decision::Qualified
    );
    assert_eq!(
        eval(&curve(2_001, 5_000, 1_000, 900), &c),
        Decision::Qualified
    );
}

// -------------------------------------------------------------------- mcap ---

#[test]
fn mcap_boundary() {
    let c = cfg();
    assert_eq!(
        reason(&curve(2_000, 1_999, 1_000, 900), &c),
        RejectReason::McapTooLow
    );
    assert_eq!(
        eval(&curve(2_000, 2_000, 1_000, 900), &c),
        Decision::Qualified
    );
    assert_eq!(
        eval(&curve(2_000, 10_000, 1_000, 900), &c),
        Decision::Qualified
    );
    assert_eq!(
        reason(&curve(2_000, 10_001, 1_000, 900), &c),
        RejectReason::McapTooHigh
    );
}

// ------------------------------------------------------------ rolling volume ---

#[test]
fn rolling_volume_boundary() {
    let c = cfg();
    assert_eq!(
        reason(&curve(2_000, 5_000, 999, 900), &c),
        RejectReason::Low5mVolume
    );
    assert_eq!(
        eval(&curve(2_000, 5_000, 1_000, 900), &c),
        Decision::Qualified
    );
    assert_eq!(
        eval(&curve(2_000, 5_000, 1_001, 900), &c),
        Decision::Qualified
    );
}

#[test]
fn rolling_volume_accumulates_multiple_trades() {
    let c = cfg();
    let mut m = curve(2_000, 5_000, 0, 900);
    m.volume.record(CUR_SLOT - 5, true, 400_000_000, 1);
    m.volume.record(CUR_SLOT - 4, false, 400_000_000, 1);
    m.volume.record(CUR_SLOT - 3, true, 200_000_000, 1);
    assert_eq!(eval(&m, &c), Decision::Qualified); // exactly $1000
}

#[test]
fn young_token_with_volume_passes() {
    // Token is 2 minutes old (300 slots) and has $1000+ rolling 5m volume.
    let m = curve(2_000, 5_000, 1_250, CUR_SLOT - 300);
    assert_eq!(eval(&m, &cfg_fresh()), Decision::Qualified);
}

#[test]
fn expired_volume_is_removed() {
    let c = cfg();
    let mut m = curve(2_000, 5_000, 0, 900);
    // 750-slot window -> a bucket at slot 249 is out of window (age 751).
    m.volume.record(CUR_SLOT - 751, true, usd(5_000) as u64, 1);
    assert_eq!(reason(&m, &c), RejectReason::Low5mVolume);
}

#[test]
fn rolling_window_boundary_is_exact() {
    let c = cfg(); // window_slots = 750 -> from_slot = 250 inclusive
    let mut inside = curve(2_000, 5_000, 0, 900);
    inside
        .volume
        .record(CUR_SLOT - 750, true, usd(1_000) as u64, 1);
    assert_eq!(eval(&inside, &c), Decision::Qualified);

    let mut outside = curve(2_000, 5_000, 0, 900);
    outside
        .volume
        .record(CUR_SLOT - 751, true, usd(1_000) as u64, 1);
    assert_eq!(reason(&outside, &c), RejectReason::Low5mVolume);
}

#[test]
fn zero_volume_rejects() {
    assert_eq!(
        reason(&curve(2_000, 5_000, 0, 900), &cfg()),
        RejectReason::Low5mVolume
    );
}

#[test]
fn high_trade_count_accumulates() {
    let c = cfg();
    let mut m = curve(2_000, 5_000, 0, 900);
    for i in 0..500u64 {
        m.volume
            .record(CUR_SLOT - 1 - (i % 100), true, 2_000_000, 1);
    }
    assert_eq!(eval(&m, &c), Decision::Qualified);
}

#[test]
fn rolling_window_is_bounded() {
    let mut m = curve(2_000, 5_000, 0, 900);
    for s in 0..5_000u64 {
        m.volume.record(s, true, 1, 1);
    }
    assert!(m.volume.len() <= 1024, "window must stay bounded");
}

// --------------------------------------------------------------- freshness ---

#[test]
fn default_max_token_age_is_15_minutes() {
    assert_eq!(StrategyConfig::default().max_token_age_seconds, Some(900));
    assert_eq!(cfg_fresh().max_age_slots(), Some(2_250)); // 900 s / 0.4 s
}

#[test]
fn freshness_zero_age_passes() {
    let m = curve(2_000, 5_000, 1_000, CUR_SLOT); // born this slot
    assert_eq!(eval(&m, &cfg_fresh()), Decision::Qualified);
}

#[test]
fn freshness_very_young_passes() {
    let m = curve(2_000, 5_000, 1_000, CUR_SLOT - 1);
    assert_eq!(eval(&m, &cfg_fresh()), Decision::Qualified);
}

#[test]
fn freshness_two_minute_token_passes() {
    // 300 slots = 120 s; with $1250 rolling volume it must remain eligible.
    let m = curve(2_000, 5_000, 1_250, CUR_SLOT - 300);
    assert_eq!(eval(&m, &cfg_fresh()), Decision::Qualified);
}

#[test]
fn freshness_exactly_15_minutes_is_inclusive() {
    // 2250 slots = exactly 900 s -> passes.
    let m = curve(2_000, 5_000, 1_000, CUR_SLOT - 2_250);
    assert_eq!(eval(&m, &cfg_fresh()), Decision::Qualified);
    assert_eq!(CUR_SLOT - (CUR_SLOT - 2_250), 2_250);
}

#[test]
fn freshness_15m_plus_one_slot_rejected() {
    // 2251 slots = 900 s + 1 slot -> rejected.
    let m = curve(2_000, 5_000, 1_000, CUR_SLOT - 2_251);
    assert_eq!(reason(&m, &cfg_fresh()), RejectReason::TokenTooOld);
}

#[test]
fn freshness_old_market_rejected() {
    let m = curve(2_000, 5_000, 1_000, 1); // ~9999 slots old
    assert_eq!(reason(&m, &cfg_fresh()), RejectReason::TokenTooOld);
}

#[test]
fn unknown_creation_fails_closed_and_is_not_inferred() {
    // created_at_slot = 0: creation was never observed -> age unknown.
    let m = curve(2_000, 5_000, 1_000, 0);
    assert_eq!(
        reason(&m, &cfg_fresh()),
        RejectReason::InsufficientFreshnessData
    );
    // Disabling the gate makes the same market evaluable again.
    assert_eq!(eval(&m, &cfg()), Decision::Qualified);
}

#[test]
fn freshness_evaluation_is_deterministic() {
    let c = cfg_fresh();
    let m = curve(2_000, 5_000, 1_000, CUR_SLOT - 2_250);
    let d = eval(&m, &c);
    for _ in 0..1000 {
        assert_eq!(eval(&m, &c), d);
    }
}

// ------------------------------------------------------------- anti-pump ---

#[test]
fn pumped_then_dumped_rejected() {
    let c = StrategyConfig {
        max_drawdown_bps: Some(3_000), // 30%
        ..cfg()
    };
    let mut m = base_market();
    m.peak_mcap_quote = usd(8_000); // peak $8k
    m.peak_mcap_quote = usd(8_000);
    // current mcap $5k -> drawdown (8000-5000)/8000 = 37.5% > 30%
    assert_eq!(reason(&m, &c), RejectReason::AlreadyPumped);
}

#[test]
fn acceptable_drawdown_passes() {
    let c = StrategyConfig {
        max_drawdown_bps: Some(5_000), // 50%
        ..cfg()
    };
    let mut m = base_market();
    m.peak_mcap_quote = usd(8_000); // 37.5% drawdown < 50%
    assert_eq!(eval(&m, &c), Decision::Qualified);
}

#[test]
fn drawdown_requires_chain_proven_history() {
    let c = StrategyConfig {
        max_drawdown_bps: Some(5_000),
        ..cfg()
    };
    // No chain-proven creation -> peak extrema cannot be trusted.
    let m = curve(2_000, 5_000, 1_000, 0);
    assert_eq!(reason(&m, &c), RejectReason::InsufficientFreshnessData);
}

#[test]
fn drawdown_no_drawdown_at_new_high() {
    let c = StrategyConfig {
        max_drawdown_bps: Some(2_000),
        ..cfg()
    };
    // peak == current (the running peak includes the current observation).
    let m = curve(2_000, 5_000, 1_000, 900);
    assert_eq!(eval(&m, &c), Decision::Qualified);
}

#[test]
fn drawdown_exact_threshold_is_inclusive() {
    // peak $10k, current $7.5k -> exactly 2500 bps.
    let mut m = curve(2_000, 7_500, 1_000, 900);
    m.peak_mcap_quote = usd(10_000);
    let exact = StrategyConfig {
        max_drawdown_bps: Some(2_500),
        ..cfg()
    };
    assert_eq!(eval(&m, &exact), Decision::Qualified);

    // One basis point tighter -> rejected.
    let tighter = StrategyConfig {
        max_drawdown_bps: Some(2_499),
        ..cfg()
    };
    assert_eq!(reason(&m, &tighter), RejectReason::AlreadyPumped);
}

#[test]
fn drawdown_one_unit_over_threshold_rejected() {
    // peak $10k, current $7.499k -> 2501 bps.
    let mut m = curve(2_000, 7_499, 1_000, 900);
    m.peak_mcap_quote = usd(10_000);
    let c = StrategyConfig {
        max_drawdown_bps: Some(2_500),
        ..cfg()
    };
    assert_eq!(reason(&m, &c), RejectReason::AlreadyPumped);
}

#[test]
fn drawdown_current_above_peak_is_zero() {
    let c = StrategyConfig {
        max_drawdown_bps: Some(1),
        ..cfg()
    };
    let mut m = curve(2_000, 5_000, 1_000, 900);
    m.peak_mcap_quote = usd(4_000); // peak below current mcap ($5k)
    assert_eq!(eval(&m, &c), Decision::Qualified);
}

#[test]
fn drawdown_zero_peak_fails_closed() {
    let c = StrategyConfig {
        max_drawdown_bps: Some(2_000),
        ..cfg()
    };
    let mut m = curve(2_000, 5_000, 1_000, 900);
    m.peak_mcap_quote = 0;
    assert_eq!(reason(&m, &c), RejectReason::InsufficientFreshnessData);
}

#[test]
fn drawdown_overflow_fails_closed() {
    let c = StrategyConfig {
        max_drawdown_bps: Some(2_000),
        ..cfg()
    };
    let mut m = curve(2_000, 5_000, 1_000, 900);
    // (peak - current) * 10_000 would overflow u128.
    m.peak_mcap_quote = u128::MAX / 2;
    assert_eq!(reason(&m, &c), RejectReason::InsufficientFreshnessData);
}

#[test]
fn drawdown_pump_then_retracement_is_detected() {
    // $3k -> $9k -> $6k is a 33.3% retracement even though $6k is in range.
    let mut m = curve(2_000, 6_000, 1_000, 900);
    m.peak_mcap_quote = usd(9_000);
    let c = StrategyConfig {
        max_drawdown_bps: Some(2_000), // 20%
        ..cfg()
    };
    assert_eq!(reason(&m, &c), RejectReason::AlreadyPumped);
}

#[test]
fn drawdown_evaluation_is_deterministic() {
    let mut m = curve(2_000, 6_000, 1_000, 900);
    m.peak_mcap_quote = usd(9_000);
    let c = StrategyConfig {
        max_drawdown_bps: Some(2_500),
        ..cfg()
    };
    let d = eval(&m, &c);
    for _ in 0..1000 {
        assert_eq!(eval(&m, &c), d);
    }
}

// ----------------------------------------------------------------- safety ---

#[test]
fn completed_curve_is_unsafe_market() {
    let mut m = base_market();
    m.complete = true;
    assert_eq!(reason(&m, &cfg()), RejectReason::UnsafeMarket);
}

#[test]
fn zero_supply_is_unsafe_token() {
    let mut m = base_market();
    m.token_total_supply = 0;
    assert_eq!(reason(&m, &cfg()), RejectReason::UnsafeToken);
}

#[test]
fn non_sol_quote_is_unsupported() {
    let mut m = base_market();
    m.quote_mint = Some(key(0xB9)); // USDC-like, not valued here
    assert_eq!(reason(&m, &cfg()), RejectReason::UnsupportedMarket);
}

#[test]
fn pumpswap_missing_mints_is_unsafe() {
    let mut m = base_market();
    m.venue = Some(Venue::PumpSwap);
    m.base_mint = None;
    m.quote_mint = Some(MarketKey([
        6, 155, 136, 87, 254, 171, 129, 132, 251, 104, 127, 99, 70, 24, 192, 53, 218, 196, 57, 220,
        26, 235, 59, 85, 152, 160, 240, 0, 0, 0, 0, 1,
    ]));
    assert_eq!(reason(&m, &cfg()), RejectReason::UnsafeMarket);
}

// ------------------------------------------------------------------ state ---

#[test]
fn state_gates() {
    let c = cfg();
    let mut unknown = base_market();
    unknown.reserves_known = false;
    assert_eq!(reason(&unknown, &c), RejectReason::StateUnknown);

    let mut stale = base_market();
    stale.last_reserve_slot = CUR_SLOT - STALE - 1;
    assert_eq!(reason(&stale, &c), RejectReason::StateStale);

    let mut invalid = base_market();
    invalid.invalidated_at_slot = Some(CUR_SLOT);
    assert_eq!(reason(&invalid, &c), RejectReason::StateInvalidated);
    assert_eq!(
        invalid.reserve_state(CUR_SLOT, STALE),
        ReserveState::Invalidated
    );
}

// -------------------------------------------------------------- buy / sell ---

#[test]
fn buy_unavailable_when_probe_yields_nothing() {
    let c = StrategyConfig {
        probe_notional_lamports: 1, // too small: net input floors to 0
        ..cfg()
    };
    assert_eq!(reason(&base_market(), &c), RejectReason::BuyUnavailable);
}

#[test]
fn sell_unavailable_when_buy_output_exceeds_u64() {
    // A huge curve so the probe's token output exceeds u64; the reverse sell
    // cannot be expressed/executed deterministically.
    let mut m = base_market();
    m.virtual_base_reserve = 2_000_000_000_000_000_000_000; // 2e21
    m.virtual_quote_reserve = 1_000_000_000;
    m.token_total_supply = 10_000_000_000_000_000_000_000; // keeps MCAP at $5k
    m.peak_mcap_quote = 0;
    let c = StrategyConfig {
        probe_notional_lamports: 1_000_000_000_000_000, // 1e15
        ..cfg()
    };
    assert_eq!(reason(&m, &c), RejectReason::SellUnavailable);
}

#[test]
fn buy_and_sell_quotes_are_available_for_a_qualified_market() {
    let m = base_market();
    let buy = quote::quote(&m, Side::Buy, 100_000_000, 95, CUR_SLOT, STALE).expect("buy quote");
    let tokens = u64::try_from(buy.net_output).unwrap();
    assert!(tokens > 0);
    let sell = quote::quote(&m, Side::Sell, tokens, 95, CUR_SLOT, STALE).expect("sell quote");
    assert!(sell.net_output > 0);
}

// -------------------------------------------------------------- economics ---

#[test]
fn economics_bounds() {
    let m = base_market();
    // Generous bounds pass.
    let ok = StrategyConfig {
        max_roundtrip_loss_bps: Some(10_000),
        max_price_impact_bps: Some(10_000),
        ..cfg()
    };
    assert_eq!(eval(&m, &ok), Decision::Qualified);

    // Zero-bound price impact always fails (impact > 0 for a finite pool).
    let strict = StrategyConfig {
        max_price_impact_bps: Some(0),
        ..cfg()
    };
    assert_eq!(reason(&m, &strict), RejectReason::BadExecutionEconomics);
}

#[test]
fn economics_roundtrip_loss_boundary() {
    let m = base_market();
    let probe = cfg().probe_notional_lamports;
    let buy = quote::quote(&m, Side::Buy, probe, 95, CUR_SLOT, STALE).unwrap();
    let tokens = u64::try_from(buy.net_output).unwrap();
    let sell = quote::quote(&m, Side::Sell, tokens, 95, CUR_SLOT, STALE).unwrap();
    let loss_bps = 10_000 - (sell.net_output * 10_000 / u128::from(probe));

    let exact = StrategyConfig {
        max_roundtrip_loss_bps: Some(loss_bps as u64),
        ..cfg()
    };
    assert_eq!(eval(&m, &exact), Decision::Qualified);

    let tighter = StrategyConfig {
        max_roundtrip_loss_bps: Some((loss_bps as u64).saturating_sub(1)),
        ..cfg()
    };
    assert_eq!(reason(&m, &tighter), RejectReason::BadExecutionEconomics);
}

// ------------------------------------------------------------------- dedup ---

#[test]
fn consumed_opportunity_cannot_requalify() {
    let mut m = base_market();
    m.strategy_version = 7;
    m.consumed_version = 7;
    assert_eq!(reason(&m, &cfg()), RejectReason::AlreadyConsumed);
}

#[test]
fn temporary_failure_can_qualify_after_state_change() {
    let c = cfg();
    let mut m = curve(2_000, 5_000, 0, 900); // no volume -> Low5mVolume
    assert_eq!(reason(&m, &c), RejectReason::Low5mVolume);
    // A new trade changes the state (and its strategy version).
    m.volume.record(CUR_SLOT, true, usd(1_000) as u64, 1);
    m.strategy_version += 1;
    assert_eq!(eval(&m, &c), Decision::Qualified);
}

// ------------------------------------------------------- engine integration ---

struct Rig {
    engine: Engine,
    handles: Vec<tokio::task::JoinHandle<()>>,
    handle: ShutdownHandle,
    metrics: Arc<Metrics>,
}

fn start_engine(cfg: StrategyConfig) -> Rig {
    let metrics = Metrics::new(8, vec![100, 1_000, 10_000, 1_000_000]);
    let (handle, shutdown) = shutdown_channel();
    let (engine, handles) =
        Engine::start_with_strategy(8, 4_096, Arc::clone(&metrics), shutdown, Arc::new(cfg), 150);
    Rig {
        engine,
        handles,
        handle,
        metrics,
    }
}

impl Rig {
    async fn market(&self, k: MarketKey) -> MarketState {
        self.engine
            .snapshot()
            .await
            .expect("snapshot")
            .into_iter()
            .flat_map(|s| s.markets)
            .find(|m| m.key == k)
            .expect("market present")
    }

    async fn shutdown(self) {
        self.handle.trigger();
        for h in self.handles {
            let _ = h.await;
        }
    }
}

fn account_event(market: MarketKey, slot: u64, wv: u64) -> NormalizedEvent {
    let decoded = DecodedAccount {
        venue: Venue::PumpFun,
        base_mint: Some(key(0xA1)),
        quote_mint: None,
        base_reserve: Some(800_000_000_000_000),
        quote_reserve: Some(5_000_000_000),
        virtual_base_reserve: Some(1_000_000_000_000_000),
        virtual_quote_reserve: Some(1_000_000_000),
        token_total_supply: Some(5_000_000_000_000_000),
        complete: Some(false),
        creator: None,
        pool_base_token_account: None,
        pool_quote_token_account: None,
    };
    NormalizedEvent::new(EventKind::Account(AccountUpdate {
        pubkey: market,
        slot,
        owner: Some(MarketKey(neurone::decode::pumpfun::PROGRAM_ID)),
        lamports: 1,
        data_len: 150,
        data_digest: wv,
        write_version: wv,
        is_startup: false,
        txn_signature: None,
        decoded: Some(decoded),
        pyth_sol_usd: None,
    }))
}

fn swap_event(market: MarketKey, slot: u64, sig: u8, quote_vol: u64) -> NormalizedEvent {
    let mut signature = [0u8; 64];
    signature[0] = sig;
    let swap = DecodedSwap {
        venue: Venue::PumpFun,
        market_key: market,
        base_mint: None,
        quote_mint: None,
        is_buy: true,
        base_amount: 10,
        quote_amount: quote_vol,
        user_quote_amount: quote_vol,
        base_reserve: Some(800_000_000_000_000),
        quote_reserve: Some(5_000_000_000),
        virtual_base_reserve: Some(1_000_000_000_000_000),
        virtual_quote_reserve: Some(1_000_000_000),
        fee_quote: 0,
        fee_bps: Some(95),
        timestamp: Some(1_700_000_000),
        ix_name: None,
    };
    NormalizedEvent::new(EventKind::Transaction(TransactionUpdate {
        signature,
        slot,
        index: 0,
        is_vote: false,
        success: true,
        keys: vec![market],
        swaps: vec![swap],
        creates: Vec::new(),
        vault_balances: Vec::new(),
        decode_rejected: 0,
        has_reserve_mutation: false,
        has_sweep: false,
    }))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_qualifies_a_market_and_counts_it_once() {
    let rig = start_engine(cfg());
    let m = key(1);
    rig.engine.route(account_event(m, 900, 1)).await.unwrap();
    rig.engine
        .route(swap_event(m, 990, 1, 1_000_000_000))
        .await
        .unwrap();

    let state = rig.market(m).await;
    assert_eq!(state.status, MarketStatus::Armed);
    let s = rig.metrics.snapshot();
    assert!(s.markets_evaluated > 0);
    assert_eq!(
        s.markets_qualified, 1,
        "newly qualified once, not repeatedly"
    );
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn independent_markets_do_not_contaminate_each_other() {
    let rig = start_engine(cfg());
    let good = key(2);
    let bad = key(3);
    rig.engine.route(account_event(good, 900, 1)).await.unwrap();
    rig.engine
        .route(swap_event(good, 990, 1, 1_000_000_000))
        .await
        .unwrap();
    // `bad` gets an account update but no volume -> not qualified.
    rig.engine.route(account_event(bad, 901, 2)).await.unwrap();

    assert_eq!(rig.market(good).await.status, MarketStatus::Armed);
    assert_eq!(rig.market(bad).await.status, MarketStatus::Observing);
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recovery_from_invalidation_requalifies() {
    let rig = start_engine(cfg());
    let m = key(4);
    rig.engine.route(account_event(m, 900, 1)).await.unwrap();
    rig.engine
        .route(swap_event(m, 990, 1, 1_000_000_000))
        .await
        .unwrap();
    assert_eq!(rig.market(m).await.status, MarketStatus::Armed);

    // A genuine reserve-mutation transaction invalidates the market.
    let mut mutation = swap_event(m, 991, 2, 0);
    if let EventKind::Transaction(t) = &mut mutation.kind {
        t.swaps.clear();
        t.has_reserve_mutation = true;
    }
    rig.engine.route(mutation).await.unwrap();
    assert_eq!(rig.market(m).await.status, MarketStatus::Observing);

    // A fresh authoritative account update re-establishes Known and re-qualifies.
    rig.engine.route(account_event(m, 992, 3)).await.unwrap();
    assert_eq!(rig.market(m).await.status, MarketStatus::Armed);
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn inactive_strategy_does_not_evaluate() {
    let rig = start_engine(StrategyConfig::default()); // no price -> inactive
    let m = key(5);
    rig.engine.route(account_event(m, 900, 1)).await.unwrap();
    rig.engine
        .route(swap_event(m, 990, 1, 1_000_000_000))
        .await
        .unwrap();
    assert_eq!(rig.market(m).await.status, MarketStatus::Observing);
    assert_eq!(rig.metrics.snapshot().markets_evaluated, 0);
    rig.shutdown().await;
}

/// A create event that proves chain-proven creation at its slot.
fn create_event(market: MarketKey, slot: u64, sig: u8) -> NormalizedEvent {
    let mut signature = [0u8; 64];
    signature[0] = sig;
    let created = CreatedMarket {
        venue: Venue::PumpFun,
        market_key: market,
        mint: key(0xA1),
        quote_mint: None,
        virtual_base_reserve: Some(1_000_000_000_000_000),
        virtual_quote_reserve: Some(1_000_000_000),
        base_reserve: Some(800_000_000_000_000),
        token_total_supply: Some(5_000_000_000_000_000),
        timestamp: Some(1_700_000_000),
    };
    NormalizedEvent::new(EventKind::Transaction(TransactionUpdate {
        signature,
        slot,
        index: 0,
        is_vote: false,
        success: true,
        keys: vec![market],
        swaps: Vec::new(),
        creates: vec![created],
        vault_balances: Vec::new(),
        decode_rejected: 0,
        has_reserve_mutation: false,
        has_sweep: false,
    }))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_fresh_market_with_proven_creation_qualifies() {
    let rig = start_engine(cfg_fresh());
    let m = key(6);
    rig.engine.route(create_event(m, 900, 1)).await.unwrap();
    let created = rig.market(m).await;
    assert_eq!(
        created.created_at_slot, 900,
        "creation slot is chain-proven"
    );
    rig.engine
        .route(swap_event(m, 990, 2, 1_000_000_000))
        .await
        .unwrap();
    assert_eq!(rig.market(m).await.status, MarketStatus::Armed);
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_late_market_without_creation_fails_closed() {
    let rig = start_engine(cfg_fresh());
    let m = key(7);
    // No create event -> creation time is unknown -> fail closed.
    rig.engine.route(account_event(m, 900, 1)).await.unwrap();
    rig.engine
        .route(swap_event(m, 990, 2, 1_000_000_000))
        .await
        .unwrap();
    assert_eq!(rig.market(m).await.status, MarketStatus::Observing);
    assert!(rig.metrics.snapshot().rejected_insufficient_freshness >= 1);
    rig.shutdown().await;
}

// --------------------------------------------------- Pyth SOL/USD reference ---

/// Pyth price = 1e11 with exponent -8 -> 1_000_000_000 micros ($1000/SOL),
/// matching the test price used by `cfg()`.
fn pyth_update(publish_time: i64) -> pyth::SolUsdPriceUpdate {
    pyth::SolUsdPriceUpdate {
        price: 100_000_000_000,
        conf: 30_000_000,
        exponent: -8,
        publish_time,
        prev_publish_time: publish_time - 1,
        ema_price: 100_000_000_000,
        ema_conf: 0,
        posted_slot: 1,
        full_verification: true,
    }
}

fn pyth_account_event(slot: u64, wv: u64, publish_time: i64) -> NormalizedEvent {
    NormalizedEvent::new(EventKind::Account(AccountUpdate {
        pubkey: MarketKey(pyth::SOL_USD_PRICE_ACCOUNT_BYTES),
        slot,
        owner: Some(MarketKey(pyth::RECEIVER_PROGRAM_ID)),
        lamports: 1,
        data_len: 134,
        data_digest: wv,
        write_version: wv,
        is_startup: false,
        txn_signature: None,
        decoded: None,
        pyth_sol_usd: Some(pyth_update(publish_time)),
    }))
}

#[test]
fn no_sol_usd_reference_fails_closed() {
    let m = base_market();
    assert_eq!(
        strategy::evaluate(&m, CUR_SLOT, STALE, &cfg(), None),
        Decision::Rejected(RejectReason::ReferenceUnavailable)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pyth_reference_updates_state_and_creates_no_market() {
    let rig = start_engine(cfg_pyth());
    rig.engine
        .route(pyth_account_event(1_000, 1, 1_791_466_553))
        .await
        .unwrap();
    let (price, _conf, publish_time, _slot, received, version) = rig.engine.sol_usd().snapshot();
    assert_eq!(price, 1_000_000_000);
    assert_eq!(publish_time, 1_791_466_553);
    assert!(received > 0);
    assert_eq!(version, 1);
    // The Pyth account is a reference feed, NOT a market.
    let markets = rig.engine.snapshot().await.unwrap();
    assert!(markets
        .iter()
        .flat_map(|s| s.markets.iter())
        .all(|m| m.key != MarketKey(pyth::SOL_USD_PRICE_ACCOUNT_BYTES)));
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pyth_mode_fails_closed_without_reference() {
    let rig = start_engine(cfg_pyth());
    let m = key(8);
    rig.engine.route(account_event(m, 900, 1)).await.unwrap();
    rig.engine
        .route(swap_event(m, 990, 1, 1_000_000_000))
        .await
        .unwrap();
    assert_eq!(rig.market(m).await.status, MarketStatus::Observing);
    assert!(rig.metrics.snapshot().rejected_state >= 1);
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pyth_mode_qualifies_with_fresh_reference() {
    let rig = start_engine(cfg_pyth());
    // Fresh reference first, then the market becomes eligible.
    rig.engine
        .route(pyth_account_event(1_000, 1, 1_791_466_553))
        .await
        .unwrap();
    let m = key(9);
    rig.engine.route(account_event(m, 900, 1)).await.unwrap();
    rig.engine
        .route(swap_event(m, 990, 1, 1_000_000_000))
        .await
        .unwrap();
    assert_eq!(rig.market(m).await.status, MarketStatus::Armed);
    assert!(rig.metrics.snapshot().markets_qualified >= 1);
    rig.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn static_mode_still_works_when_explicitly_selected() {
    // cfg() is Static with an explicit operator price.
    let rig = start_engine(cfg());
    let m = key(10);
    rig.engine.route(account_event(m, 900, 1)).await.unwrap();
    rig.engine
        .route(swap_event(m, 990, 1, 1_000_000_000))
        .await
        .unwrap();
    assert_eq!(rig.market(m).await.status, MarketStatus::Armed);
    assert_eq!(
        rig.engine.sol_usd().snapshot().0,
        0,
        "static mode uses no feed"
    );
    rig.shutdown().await;
}
