use serde::Deserialize;

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TradingMode {
    Paper,
    Live,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TradingConfig {
    pub mode: TradingMode,
    pub fixed_amount_krw: i64,
    pub breakout_buffer_pct: f64,
    pub range_minutes: u32,
    pub poll_interval_secs: u64,
    pub exit_time: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RiskConfig {
    pub stop_loss_pct: f64,
    pub daily_loss_limit_krw: i64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SymbolsConfig {
    pub watchlist: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub trading: TradingConfig,
    pub risk: RiskConfig,
    pub symbols: SymbolsConfig,
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> anyhow::Result<()> {
        // Validate exit_time format
        let parts: Vec<&str> = self.trading.exit_time.splitn(2, ':').collect();
        if parts.len() != 2 {
            anyhow::bail!("invalid exit_time '{}': must be HH:MM format", self.trading.exit_time);
        }
        let h: u32 = parts[0].parse().map_err(|_| anyhow::anyhow!("invalid exit_time hour '{}'", parts[0]))?;
        let m: u32 = parts[1].parse().map_err(|_| anyhow::anyhow!("invalid exit_time minute '{}'", parts[1]))?;
        if h > 23 || m > 59 {
            anyhow::bail!("invalid exit_time '{}': hour must be 0-23, minute 0-59", self.trading.exit_time);
        }
        // Validate watchlist is non-empty
        if self.symbols.watchlist.is_empty() {
            anyhow::bail!("watchlist must not be empty");
        }
        // Validate positive amounts
        if self.trading.fixed_amount_krw <= 0 {
            anyhow::bail!("fixed_amount_krw must be positive");
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Credentials {
    pub app_key: String,
    pub app_secret: String,
    pub account_no: String,
}

impl Credentials {
    /// Load from environment variables. Panics with a clear message if any are missing.
    pub fn from_env() -> Self {
        let app_key = std::env::var("KIS_APP_KEY")
            .expect("KIS_APP_KEY not set — copy .env.example to .env and fill in your credentials");
        let app_secret = std::env::var("KIS_APP_SECRET")
            .expect("KIS_APP_SECRET not set — copy .env.example to .env and fill in your credentials");
        let account_no = std::env::var("KIS_ACCOUNT_NO")
            .expect("KIS_ACCOUNT_NO not set — copy .env.example to .env and fill in your credentials");
        Self { app_key, app_secret, account_no }
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
        assert_eq!(config.trading.fixed_amount_krw, 500_000);
        assert!((config.trading.breakout_buffer_pct - 0.2).abs() < f64::EPSILON);
        assert_eq!(config.trading.range_minutes, 30);
        assert_eq!(config.trading.poll_interval_secs, 5);
        assert_eq!(config.trading.exit_time, "15:20");
        assert!((config.risk.stop_loss_pct - 1.5).abs() < f64::EPSILON);
        assert_eq!(config.risk.daily_loss_limit_krw, 100_000);
        assert_eq!(config.symbols.watchlist, vec!["005930", "069500"]);
    }

    #[test]
    fn test_credentials_from_env() {
        unsafe {
            std::env::set_var("KIS_APP_KEY", "test_key");
            std::env::set_var("KIS_APP_SECRET", "test_secret");
            std::env::set_var("KIS_ACCOUNT_NO", "12345678");
        }
        let creds = Credentials::from_env();
        assert_eq!(creds.app_key, "test_key");
        assert_eq!(creds.app_secret, "test_secret");
        assert_eq!(creds.account_no, "12345678");
    }
}
