use chrono::NaiveTime;
use serde::Deserialize;

fn deserialize_naive_time<'de, D>(de: D) -> Result<NaiveTime, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(de)?;
    NaiveTime::parse_from_str(&s, "%H:%M").map_err(serde::de::Error::custom)
}

fn deserialize_tz<'de, D>(de: D) -> Result<chrono_tz::Tz, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(de)?;
    s.parse::<chrono_tz::Tz>().map_err(serde::de::Error::custom)
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TradingMode {
    Paper,
    Live,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TradingConfig {
    pub mode: TradingMode,
    pub fixed_amount: i64,
    pub breakout_buffer_pct: f64,
    pub range_minutes: u32,
    pub poll_interval_secs: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RiskConfig {
    pub stop_loss_pct: f64,
    pub daily_loss_limit: i64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MarketConfig {
    #[serde(deserialize_with = "deserialize_tz")]
    pub timezone: chrono_tz::Tz,
    #[serde(deserialize_with = "deserialize_naive_time")]
    pub open_time: NaiveTime,
    #[serde(deserialize_with = "deserialize_naive_time")]
    pub exit_time: NaiveTime,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SymbolConfig {
    pub ticker: String,
    pub fixed_amount: Option<i64>,
    pub breakout_buffer_pct: Option<f64>,
    pub stop_loss_pct: Option<f64>,
}

impl SymbolConfig {
    pub fn effective_fixed_amount(&self, trading: &TradingConfig) -> i64 {
        self.fixed_amount.unwrap_or(trading.fixed_amount)
    }

    pub fn effective_breakout_buffer_pct(&self, trading: &TradingConfig) -> f64 {
        self.breakout_buffer_pct
            .unwrap_or(trading.breakout_buffer_pct)
    }

    pub fn effective_stop_loss_pct(&self, risk: &RiskConfig) -> f64 {
        self.stop_loss_pct.unwrap_or(risk.stop_loss_pct)
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StrategyConfig {
    Orb,
    EmaCross { fast_period: u32, slow_period: u32 },
    VwapReversion { entry_deviation_pct: f64 },
}

impl Default for StrategyConfig {
    fn default() -> Self {
        StrategyConfig::Orb
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
}

fn default_log_level() -> String {
    "info".to_string()
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub trading: TradingConfig,
    pub risk: RiskConfig,
    pub market: MarketConfig,
    #[serde(default)]
    pub strategy: StrategyConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    pub symbols: Vec<SymbolConfig>,
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.symbols.is_empty() {
            anyhow::bail!("symbols list must not be empty");
        }
        if self.trading.fixed_amount <= 0 {
            anyhow::bail!("fixed_amount must be positive");
        }
        if self.market.exit_time <= self.market.open_time {
            anyhow::bail!("exit_time must be after open_time");
        }
        Ok(())
    }

    pub fn tickers(&self) -> Vec<String> {
        self.symbols.iter().map(|s| s.ticker.clone()).collect()
    }
}

#[derive(Debug, Clone)]
pub struct KisCredentials {
    pub app_key: String,
    pub app_secret: String,
    pub account_no: String,
}

impl KisCredentials {
    pub fn from_env() -> Self {
        let app_key = std::env::var("KIS_APP_KEY")
            .expect("KIS_APP_KEY not set — copy .env.example to .env and fill in your credentials");
        let app_secret = std::env::var("KIS_APP_SECRET").expect(
            "KIS_APP_SECRET not set — copy .env.example to .env and fill in your credentials",
        );
        let account_no = std::env::var("KIS_ACCOUNT_NO").expect(
            "KIS_ACCOUNT_NO not set — copy .env.example to .env and fill in your credentials",
        );
        Self {
            app_key,
            app_secret,
            account_no,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_config() {
        let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let config = Config::load(manifest.join("config.toml").to_str().unwrap())
            .expect("should load config.toml");
        assert_eq!(config.trading.mode, TradingMode::Paper);
        assert_eq!(config.trading.fixed_amount, 500_000);
        assert!((config.trading.breakout_buffer_pct - 0.2).abs() < f64::EPSILON);
        assert_eq!(config.trading.range_minutes, 30);
        assert_eq!(config.trading.poll_interval_secs, 5);
        assert_eq!(config.market.timezone, chrono_tz::Asia::Seoul);
        assert_eq!(
            config.market.open_time,
            NaiveTime::from_hms_opt(9, 0, 0).unwrap()
        );
        assert_eq!(
            config.market.exit_time,
            NaiveTime::from_hms_opt(15, 20, 0).unwrap()
        );
        assert!((config.risk.stop_loss_pct - 5.0).abs() < f64::EPSILON);
        assert_eq!(config.risk.daily_loss_limit, 100_000);
        assert!(matches!(config.strategy, StrategyConfig::Orb));
        assert_eq!(config.tickers(), vec!["005930"]);
    }

    #[test]
    fn test_kis_credentials_from_env() {
        unsafe {
            std::env::set_var("KIS_APP_KEY", "test_key");
            std::env::set_var("KIS_APP_SECRET", "test_secret");
            std::env::set_var("KIS_ACCOUNT_NO", "12345678");
        }
        let creds = KisCredentials::from_env();
        assert_eq!(creds.app_key, "test_key");
        assert_eq!(creds.app_secret, "test_secret");
        assert_eq!(creds.account_no, "12345678");
    }
}
