/// Backtest runner binary.
///
/// Usage:
///   cargo run --bin backtest -- <YYYY-MM-DD>                   # single day
///   cargo run --bin backtest -- <YYYY-MM-DD> <YYYY-MM-DD>     # date range
///
/// Fetches minute-bar data from KIS for each trading day in the range (cached
/// under data/YYYYMMDD/<symbol>.csv), then runs all strategy variants over the
/// same tick stream and prints a comparison table.
use std::sync::Arc;
use chrono::NaiveDate;

use protrader::auth::KisAuthProvider;
use protrader::backtest::BacktestRunner;
use protrader::config::{Config, KisCredentials};
use protrader::historical::KisHistoricalClient;
use protrader::strategy::{EmaCrossStrategy, OrbStrategy, VwapReversionStrategy};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    let mut args = std::env::args().skip(1);
    let start_str = args.next().unwrap_or_else(|| {
        eprintln!("Usage: backtest <YYYY-MM-DD> [<YYYY-MM-DD>]");
        std::process::exit(1);
    });
    let start = NaiveDate::parse_from_str(&start_str, "%Y-%m-%d")
        .map_err(|e| anyhow::anyhow!("invalid start date '{}': {}", start_str, e))?;
    let end = match args.next() {
        Some(s) => NaiveDate::parse_from_str(&s, "%Y-%m-%d")
            .map_err(|e| anyhow::anyhow!("invalid end date '{}': {}", s, e))?,
        None => start,
    };
    anyhow::ensure!(end >= start, "end date must be >= start date");

    let config = Arc::new(Config::load("config.toml")?);

    let credentials = KisCredentials::from_env();
    let auth = KisAuthProvider::new(
        protrader::http_client(),
        "https://openapi.koreainvestment.com:9443".to_string(),
        credentials,
    )
    .await?;

    // ── Fetch historical minute bars ──────────────────────────────────────────

    let hist = KisHistoricalClient::new(auth);
    let mut ticks = Vec::new();
    for sc in &config.symbols {
        if start == end {
            println!("Fetching {} for {}...", sc.ticker, start);
            let mut t = hist.fetch_day(&sc.ticker, start).await?;
            println!("  {} ticks", t.len());
            ticks.append(&mut t);
        } else {
            println!("Fetching {} for {} → {}...", sc.ticker, start, end);
            let mut t = hist.fetch_range(&sc.ticker, start, end).await?;
            println!("  {} ticks across date range", t.len());
            ticks.append(&mut t);
        }
    }
    ticks.sort_by_key(|t| t.time);

    if ticks.is_empty() {
        anyhow::bail!("no data returned — check that the date range contains valid KST trading days");
    }

    // ── Build strategy variants ───────────────────────────────────────────────

    let mut runner = BacktestRunner::new(config.clone());

    // Variant 1: default params straight from config.toml
    runner.add_run(
        "ORB-default",
        Box::new(OrbStrategy::new(&config.trading, &config.risk, &config.symbols)),
        10_000_000,
    );

    // Variant 2: tighter breakout buffer — triggers faster but more false signals
    let mut t2 = config.trading.clone();
    t2.breakout_buffer_pct = 0.1;
    runner.add_run(
        "ORB-tight-buffer",
        Box::new(OrbStrategy::new(&t2, &config.risk, &config.symbols)),
        10_000_000,
    );

    // Variant 3: wider stop loss — gives trades more room before stopping out
    let mut r3 = config.risk.clone();
    r3.stop_loss_pct = 3.0;
    runner.add_run(
        "ORB-wide-stoploss",
        Box::new(OrbStrategy::new(&config.trading, &r3, &config.symbols)),
        10_000_000,
    );

    // Variant 4: EMA crossover — fast=5, slow=20 (intraday momentum)
    runner.add_run(
        "EMA-cross-5-20",
        Box::new(EmaCrossStrategy::new(
            &config.trading,
            &config.risk,
            &config.symbols,
            5,
            20,
        )),
        10_000_000,
    );

    // Variant 5: EMA crossover — faster periods for more responsive signals
    runner.add_run(
        "EMA-cross-3-10",
        Box::new(EmaCrossStrategy::new(
            &config.trading,
            &config.risk,
            &config.symbols,
            3,
            10,
        )),
        10_000_000,
    );

    // Variant 6: VWAP mean reversion — buy 1% below running VWAP, exit on reversion
    runner.add_run(
        "VWAP-reversion-1pct",
        Box::new(VwapReversionStrategy::new(
            &config.trading,
            &config.risk,
            &config.symbols,
            1.0,
        )),
        10_000_000,
    );

    // Variant 7: VWAP mean reversion — tighter entry (0.5%) catches shallower dips
    runner.add_run(
        "VWAP-reversion-0.5pct",
        Box::new(VwapReversionStrategy::new(
            &config.trading,
            &config.risk,
            &config.symbols,
            0.5,
        )),
        10_000_000,
    );

    // ── Run and print results ─────────────────────────────────────────────────

    println!(
        "\nRunning {} strategies over {} ticks ({} → {})...\n",
        runner.run_count(),
        ticks.len(),
        start,
        end,
    );

    let results = if start == end {
        runner.run(&ticks).await
    } else {
        runner.run_days(&ticks).await
    };

    println!(
        "{:<25} {:>14} {:>14} {:>8}",
        "Strategy", "Realized P&L", "Total P&L", "Trades"
    );
    println!("{}", "─".repeat(65));
    for r in &results {
        println!(
            "{:<25} {:>14} {:>14} {:>8}",
            r.name,
            format_krw(r.realized_pnl),
            format_krw(r.realized_pnl + r.unrealized_pnl),
            r.trade_count,
        );
    }

    Ok(())
}

fn format_krw(amount: i64) -> String {
    if amount >= 0 {
        format!("+₩{}", amount)
    } else {
        format!("-₩{}", -amount)
    }
}
