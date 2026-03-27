/// Backtest runner binary.
///
/// Usage:
///   cargo run --bin backtest -- <YYYY-MM-DD>
///
/// Fetches minute-bar data from KIS for the given date (cached under data/),
/// then runs all registered strategy variants in parallel over the same tick
/// stream and prints a comparison table.
use std::sync::Arc;
use chrono::NaiveDate;

use protrader::auth::KisAuthProvider;
use protrader::backtest::BacktestRunner;
use protrader::config::{Config, KisCredentials};
use protrader::historical::KisHistoricalClient;
use protrader::strategy::OrbStrategy;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    let date_str = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: backtest <YYYY-MM-DD>");
        std::process::exit(1);
    });
    let date = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d")
        .map_err(|e| anyhow::anyhow!("invalid date '{}': {}", date_str, e))?;

    let config = Arc::new(Config::load("config.toml")?);

    let credentials = KisCredentials::from_env();
    let auth = KisAuthProvider::new(
        reqwest::Client::new(),
        "https://openapi.koreainvestment.com:9443".to_string(),
        credentials,
    )
    .await?;

    // ── Fetch historical minute bars ──────────────────────────────────────────

    let hist = KisHistoricalClient::new(auth);
    let mut ticks = Vec::new();
    for sc in &config.symbols {
        println!("Fetching {} for {}...", sc.ticker, date);
        let mut t = hist.fetch_day(&sc.ticker, date).await?;
        println!("  {} ticks (cached at data/{}/{}.csv)", t.len(), date.format("%Y%m%d"), sc.ticker);
        ticks.append(&mut t);
    }
    // Interleave all symbols in time order for the runner
    ticks.sort_by_key(|t| t.time);

    if ticks.is_empty() {
        anyhow::bail!("no data returned — is {} a valid KST trading day?", date);
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

    // ── Run and print results ─────────────────────────────────────────────────

    println!("\nRunning {} strategies over {} ticks...\n", runner.run_count(), ticks.len());
    let results = runner.run(&ticks).await;

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
