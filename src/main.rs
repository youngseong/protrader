use std::sync::Arc;
use tokio::sync::Mutex;

use protrader::auth::KisAuthProvider;
use protrader::config::{Config, KisCredentials, TradingMode};
use protrader::market_data::KisMarketDataClient;
use protrader::order::{LiveOrderClient, PaperOrderClient};
use protrader::scheduler::SessionScheduler;
use protrader::strategy::{OrbStrategy, StrategyEngine};
use protrader::telegram::TelegramNotifier;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    let _guard = protrader::logging::init();

    let config = Arc::new(Config::load("config.toml")?);
    tracing::info!("Config loaded — mode={:?}", config.trading.mode);

    let credentials = KisCredentials::from_env();
    let auth = KisAuthProvider::new(
        reqwest::Client::new(),
        "https://openapi.koreainvestment.com:9443".to_string(),
        credentials,
    )
    .await?;

    let market_data = Arc::new(KisMarketDataClient::new(auth.clone()));

    let orb = OrbStrategy::new(&config.trading, &config.risk, &config.symbols);
    let engine = Arc::new(Mutex::new(StrategyEngine::new(
        Box::new(orb),
        config.risk.daily_loss_limit,
    )));

    let notifier = TelegramNotifier::from_env().map(Arc::new);
    if notifier.is_some() {
        tracing::info!("Telegram notifications enabled");
    }

    match config.trading.mode {
        TradingMode::Paper => {
            tracing::info!("Running in PAPER mode — no real orders will be placed");
            let order_client = Arc::new(PaperOrderClient);
            SessionScheduler::new(config, engine, market_data, order_client, notifier)
                .run()
                .await?;
        }
        TradingMode::Live => {
            tracing::info!("Running in LIVE mode — real orders WILL be placed");
            let order_client = Arc::new(LiveOrderClient::new(auth));
            SessionScheduler::new(config, engine, market_data, order_client, notifier)
                .run()
                .await?;
        }
    }

    Ok(())
}
