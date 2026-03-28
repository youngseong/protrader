use chrono::NaiveDate;
/// Fetch-range profiler.
///
/// Measures wall-clock time for `fetch_range` over 2026-03-02 → 2026-03-15
/// for every symbol in config.toml.  Prints per-symbol and total elapsed times.
///
///   cargo run --bin profile_fetch --release
use std::sync::Arc;

use protrader::auth::KisAuthProvider;
use protrader::config::{Config, KisCredentials};
use protrader::historical::KisHistoricalClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    let config = Arc::new(Config::load("config.toml")?);
    let credentials = KisCredentials::from_env();
    let auth = KisAuthProvider::new(
        reqwest::Client::new(),
        "https://openapi.koreainvestment.com:9443".to_string(),
        credentials,
    )
    .await?;

    let hist = KisHistoricalClient::new(auth);
    let start = NaiveDate::from_ymd_opt(2026, 3, 2).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 3, 27).unwrap();

    println!(
        "Profiling fetch_range({} → {}) for {} symbol(s)\n",
        start,
        end,
        config.symbols.len()
    );
    println!(
        "{:<12} {:>10} {:>10} {:>12}",
        "Symbol", "Ticks", "Days", "Elapsed"
    );
    println!("{}", "─".repeat(48));

    let wall_start = std::time::Instant::now();

    for sc in &config.symbols {
        let t0 = std::time::Instant::now();
        let ticks = hist.fetch_range(&sc.ticker, start, end).await?;
        let elapsed = t0.elapsed();

        // Count distinct calendar days present in the results.
        let mut days: Vec<_> = ticks.iter().map(|t| t.time.date()).collect();
        days.dedup();

        println!(
            "{:<12} {:>10} {:>10} {:>11.2}s",
            sc.ticker,
            ticks.len(),
            days.len(),
            elapsed.as_secs_f64()
        );
    }

    println!("{}", "─".repeat(48));
    println!(
        "{:<12} {:>33.2}s",
        "TOTAL",
        wall_start.elapsed().as_secs_f64()
    );

    Ok(())
}
