use std::sync::Arc;
use tokio::sync::Mutex;

use protrader::auth::KisAuthProvider;
use protrader::config::{Config, KisCredentials, TradingMode};
use protrader::historical::KisHistoricalClient;
use protrader::market_data::KisMarketDataClient;
use protrader::order::{LiveOrderClient, PaperOrderClient};
use protrader::scheduler::SessionScheduler;
use protrader::config::StrategyConfig;
use protrader::strategies::{EmaCrossStrategy, OrbStrategy, StrategyEngine, VwapReversionStrategy};
use protrader::telegram::TelegramNotifier;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    let config = Arc::new(Config::load("config.toml")?);
    let _guard = protrader::logging::init(&config.logging.level);
    tracing::info!("Config loaded — mode={:?}, log_level={}", config.trading.mode, config.logging.level);

    let credentials = KisCredentials::from_env();
    let auth = KisAuthProvider::new(
        protrader::http_client(),
        "https://openapi.koreainvestment.com:9443".to_string(),
        credentials,
    )
    .await?;

    let market_data = Arc::new(KisMarketDataClient::new(auth.clone()));
    let historical = Arc::new(KisHistoricalClient::new(auth.clone()));

    let strategy: Box<dyn protrader::strategies::Strategy> = match &config.strategy {
        StrategyConfig::Orb => Box::new(OrbStrategy::new(&config.trading, &config.risk, &config.symbols)),
        StrategyConfig::EmaCross { fast_period, slow_period } => Box::new(EmaCrossStrategy::new(
            &config.trading, &config.risk, &config.symbols, *fast_period, *slow_period,
        )),
        StrategyConfig::VwapReversion { entry_deviation_pct } => Box::new(VwapReversionStrategy::new(
            &config.trading, &config.risk, &config.symbols, *entry_deviation_pct,
        )),
    };
    tracing::info!("Strategy: {:?}", config.strategy);
    let engine = Arc::new(Mutex::new(StrategyEngine::new(strategy, config.risk.daily_loss_limit)));

    let notifier = TelegramNotifier::from_env().map(Arc::new);
    if notifier.is_some() {
        tracing::info!("Telegram notifications enabled");
    }

    match config.trading.mode {
        TradingMode::Paper => {
            tracing::info!("Running in PAPER mode — no real orders will be placed");
            let order_client = Arc::new(PaperOrderClient::new(10_000_000));
            SessionScheduler::new(config, engine, market_data, order_client, notifier, Some(historical))
                .run()
                .await?;
        }
        TradingMode::Live => {
            tracing::info!("Running in LIVE mode — real orders WILL be placed");
            let order_client = Arc::new(LiveOrderClient::new(auth));
            SessionScheduler::new(config, engine, market_data, order_client, notifier, Some(historical))
                .run()
                .await?;
        }
    }

    Ok(())
}
