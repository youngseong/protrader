use std::sync::Arc;
use tokio::sync::Mutex;

use protrader::config::{Config, Credentials, TradingMode};
use protrader::market_data::KisMarketDataClient;
use protrader::order::{LiveOrderClient, PaperOrderClient};
use protrader::scheduler::SessionScheduler;
use protrader::strategy::StrategyEngine;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env if present; ignore if missing
    let _ = dotenvy::dotenv();

    let _guard = protrader::logging::init();

    let config = Arc::new(Config::load("config.toml")?);
    tracing::info!("Config loaded — mode={:?}", config.trading.mode);

    let credentials = Credentials::from_env();

    let engine = Arc::new(Mutex::new(StrategyEngine::new(
        config.trading.clone(),
        config.risk.clone(),
        &config.symbols.watchlist,
    )));

    match config.trading.mode {
        TradingMode::Paper => {
            tracing::info!("Running in PAPER mode — no real orders will be placed");
            let market_data = Arc::new(KisMarketDataClient::new(credentials).await?);
            let order_client = Arc::new(PaperOrderClient);
            SessionScheduler::new(config, engine, market_data, order_client)
                .run()
                .await?;
        }
        TradingMode::Live => {
            tracing::info!("Running in LIVE mode — real orders WILL be placed");
            let market_data = Arc::new(KisMarketDataClient::new(credentials.clone()).await?);
            let order_client = Arc::new(LiveOrderClient::new(credentials).await?);
            SessionScheduler::new(config, engine, market_data, order_client)
                .run()
                .await?;
        }
    }

    Ok(())
}
