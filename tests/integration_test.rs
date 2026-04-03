use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::Mutex;

use protrader::config::{Config, TradingConfig, RiskConfig, MarketConfig, StrategyConfig, SymbolConfig, TradingMode};
use protrader::strategy::{OrbStrategy, StrategyEngine, SessionPhase, Signal, ExitReason};
use protrader::market_data::{MockMarketDataClient, MarketDataClient};

fn sym(ticker: &str) -> SymbolConfig {
    SymbolConfig {
        ticker: ticker.to_string(),
        fixed_amount: None,
        breakout_buffer_pct: None,
        stop_loss_pct: None,
    }
}

fn paper_config(tickers: Vec<&str>) -> Config {
    Config {
        trading: TradingConfig {
            mode: TradingMode::Paper,
            fixed_amount: 500_000,
            breakout_buffer_pct: 0.2,
            range_minutes: 30,
            poll_interval_secs: 0,
        },
        risk: RiskConfig {
            stop_loss_pct: 1.5,
            daily_loss_limit: 100_000,
        },
        market: MarketConfig {
            timezone: chrono_tz::Asia::Seoul,
            open_time: chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            exit_time: chrono::NaiveTime::from_hms_opt(15, 20, 0).unwrap(),
        },
        strategy: StrategyConfig::Orb,
        symbols: tickers.into_iter().map(sym).collect(),
    }
}

fn make_engine(config: &Config) -> Arc<Mutex<StrategyEngine>> {
    let orb = OrbStrategy::new(&config.trading, &config.risk, &config.symbols);
    Arc::new(Mutex::new(StrategyEngine::new(Box::new(orb), config.risk.daily_loss_limit)))
}

/// Full session: range capture → breakout buy → forced close at end of day.
/// Prices: 71_000, 71_500, 72_000 (range), 72_100 (below breakout), 73_000 (buy), 73_500 (exit)
#[tokio::test]
async fn test_full_session_buy_and_forced_close() {
    let config = paper_config(vec!["005930"]);
    let engine = make_engine(&config);

    let mut price_map = HashMap::new();
    price_map.insert(
        "005930".to_string(),
        vec![71_000i64, 71_500, 72_000, 72_100, 73_000, 73_500],
    );
    let market_data = Arc::new(MockMarketDataClient::new(price_map));

    // ── Range capture: 3 ticks ───────────────────────────────────────────────
    for _ in 0..3 {
        let q = market_data.fetch_price("005930").await.unwrap();
        engine.lock().await.on_tick("005930", q.price);
    }

    // ── Switch to Monitoring ─────────────────────────────────────────────────
    engine.lock().await.set_phase(SessionPhase::Monitoring);

    // Tick at 72_100 — below breakout (72_000 * 1.002 = 72_144) → Hold
    let q = market_data.fetch_price("005930").await.unwrap();
    let signal = engine.lock().await.on_tick("005930", q.price);
    assert_eq!(signal, Signal::Hold);

    // Tick at 73_000 — above breakout → Buy; qty = 500_000 / 73_000 = 6
    let q = market_data.fetch_price("005930").await.unwrap();
    let signal = engine.lock().await.on_tick("005930", q.price);
    assert_eq!(signal, Signal::Buy { price: 73_000, qty: 6 });
    engine.lock().await.record_buy("005930", 73_000, 6);

    // ── Switch to Closed (end of day) ────────────────────────────────────────
    engine.lock().await.set_phase(SessionPhase::Closed);

    // Tick at 73_500 → ForcedClose
    let q = market_data.fetch_price("005930").await.unwrap();
    let signal = engine.lock().await.on_tick("005930", q.price);
    assert_eq!(signal, Signal::Exit { price: 73_500, reason: ExitReason::ForcedClose });

    let pnl = engine.lock().await.record_exit("005930", 73_500, false);
    assert_eq!(pnl, (73_500 - 73_000) * 6); // +3_000

    let session = engine.lock().await.session_pnl();
    assert_eq!(session.realized, 3_000);
    assert_eq!(session.total(), 3_000);
}

/// Stop-loss fires, symbol is blacklisted, no re-entry even on subsequent breakout.
#[tokio::test]
async fn test_stop_loss_blacklists_and_prevents_reentry() {
    let config = paper_config(vec!["005930"]);
    let engine = make_engine(&config);

    // Range: single tick sets range_high = 71_000
    engine.lock().await.on_tick("005930", 71_000);
    engine.lock().await.set_phase(SessionPhase::Monitoring);
    engine.lock().await.record_buy("005930", 72_000, 6);

    // Stop-loss fires: 72_000 * (1 - 0.015) = 70_920; 70_800 <= 70_920
    let signal = engine.lock().await.on_tick("005930", 70_800);
    assert_eq!(signal, Signal::Exit { price: 70_800, reason: ExitReason::StopLoss });

    let pnl = engine.lock().await.record_exit("005930", 70_800, true); // blacklist=true
    assert_eq!(pnl, (70_800 - 72_000) * 6); // -7_200

    // After blacklisting, no re-entry even on a strong breakout price
    let signal = engine.lock().await.on_tick("005930", 80_000);
    assert_eq!(signal, Signal::Hold);

    let session = engine.lock().await.session_pnl();
    assert_eq!(session.realized, -7_200);
}

/// After daily loss limit is hit, no new buy entries are allowed on any symbol.
#[tokio::test]
async fn test_daily_loss_limit_stops_all_new_entries() {
    let config = paper_config(vec!["005930", "069500"]);
    let engine = make_engine(&config);

    // Set up ranges for both symbols
    engine.lock().await.on_tick("005930", 71_000);
    engine.lock().await.on_tick("069500", 9_000);
    engine.lock().await.set_phase(SessionPhase::Monitoring);

    // Buy 005930 then take a large loss exceeding the daily limit
    engine.lock().await.record_buy("005930", 72_000, 6);
    // Loss: (55_000 - 72_000) * 6 = -102_000 (exceeds 100_000 limit)
    engine.lock().await.record_exit("005930", 55_000, false);
    assert!(engine.lock().await.daily_limit_hit());

    // 069500 would break out (9_000 * 1.002 = 9_018; price 9_500 > 9_018)
    // but daily limit is hit → Hold (no open position to close)
    let signal = engine.lock().await.on_tick("069500", 9_500);
    assert_eq!(signal, Signal::Hold);
}

/// P&L tracking: unrealized updates on tick, realized on close.
#[tokio::test]
async fn test_pnl_tracking_unrealized_and_realized() {
    let config = paper_config(vec!["005930"]);
    let engine = make_engine(&config);

    engine.lock().await.on_tick("005930", 71_000);
    engine.lock().await.set_phase(SessionPhase::Monitoring);
    engine.lock().await.record_buy("005930", 72_000, 6);

    // Price tick at 73_000: unrealized = (73_000 - 72_000) * 6 = 6_000
    engine.lock().await.on_tick("005930", 73_000);
    let session = engine.lock().await.session_pnl();
    assert_eq!(session.unrealized, 6_000);
    assert_eq!(session.realized, 0);

    // Close at 73_500: realized = (73_500 - 72_000) * 6 = 9_000
    engine.lock().await.record_exit("005930", 73_500, false);
    let session = engine.lock().await.session_pnl();
    assert_eq!(session.realized, 9_000);
    assert_eq!(session.unrealized, 0);
    assert_eq!(session.total(), 9_000);
}
