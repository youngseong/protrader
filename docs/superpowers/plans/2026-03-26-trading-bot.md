# ProTrader — KIS Opening Range Breakout Bot Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Rust tokio-based KIS Opening Range Breakout bot with paper/live modes, P&L tracking, stop-loss, and daily loss limit.

**Architecture:** Single async binary; four separated layers (Scheduler, StrategyEngine, MarketDataClient, OrderClient); one tokio task per symbol; shared engine behind `Arc<Mutex<>>`; phase transitions (CapturingRange → Monitoring → Closed) controlled by Scheduler.

**Tech Stack:** Rust 2021, tokio (full), reqwest (json), serde (derive) + toml, dotenvy, tracing + tracing-subscriber + tracing-appender, chrono + chrono-tz, anyhow, async-trait

---

## File Map

| File | Responsibility |
|---|---|
| `Cargo.toml` | All dependencies; lib + bin targets |
| `config.toml` | Non-sensitive runtime settings |
| `.env.example` | Credential template |
| `.gitignore` | Excludes `.env`, `logs/`, `target/` |
| `src/lib.rs` | Re-exports all modules (needed for integration tests) |
| `src/main.rs` | Entry point: load `.env`, validate credentials, run scheduler |
| `src/config.rs` | `Config`, `TradingConfig`, `RiskConfig`, `SymbolsConfig`, `Credentials`, `TradingMode` |
| `src/logging.rs` | Tracing subscriber: stdout + daily rotating file |
| `src/strategy.rs` | `StrategyEngine`, `Signal`, `ExitReason`, `Position`, `SymbolState`, `SessionPhase`, `SessionPnl` |
| `src/order.rs` | `OrderClient` trait, `PaperOrderClient`, `LiveOrderClient`, `OrderRequest`, `OrderSide` |
| `src/market_data.rs` | `MarketDataClient` trait, `KisMarketDataClient`, `MockMarketDataClient` |
| `src/scheduler.rs` | `SessionScheduler`: session clock, phase transitions, per-symbol task loop |
| `tests/integration_test.rs` | Full simulated session with mock clients |

---

### Task 1: Project Scaffold

**Files:**
- Create: `Cargo.toml`
- Create: `config.toml`
- Create: `.env.example`
- Create: `.gitignore`
- Create: `src/lib.rs`
- Create: `src/main.rs`

- [ ] **Step 1: Create `Cargo.toml`**

```toml
[package]
name = "protrader"
version = "0.1.0"
edition = "2021"

[lib]
name = "protrader"
path = "src/lib.rs"

[[bin]]
name = "protrader"
path = "src/main.rs"

[dependencies]
tokio = { version = "1", features = ["full"] }
reqwest = { version = "0.12", features = ["json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
dotenvy = "0.15"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
tracing-appender = "0.2"
chrono = { version = "0.4", features = ["serde"] }
chrono-tz = "0.9"
anyhow = "1"
async-trait = "0.1"
```

- [ ] **Step 2: Create `config.toml`**

```toml
[trading]
mode = "paper"
fixed_amount_krw = 500000
breakout_buffer_pct = 0.2
range_minutes = 30
poll_interval_secs = 5
exit_time = "15:20"

[risk]
stop_loss_pct = 1.5
daily_loss_limit_krw = 100000

[symbols]
watchlist = ["005930", "069500"]
```

- [ ] **Step 3: Create `.env.example`**

```
KIS_APP_KEY=your_app_key_here
KIS_APP_SECRET=your_app_secret_here
KIS_ACCOUNT_NO=your_account_number_here
```

- [ ] **Step 4: Create `.gitignore`**

```
/target
.env
logs/
```

- [ ] **Step 5: Create `src/lib.rs`**

```rust
pub mod config;
pub mod logging;
pub mod market_data;
pub mod order;
pub mod scheduler;
pub mod strategy;
```

- [ ] **Step 6: Create `src/main.rs` (stub that compiles)**

```rust
fn main() {
    println!("ProTrader starting...");
}
```

- [ ] **Step 7: Verify it compiles**

Run: `cargo build`
Expected: compiles with no errors (lib and bin)

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml config.toml .env.example .gitignore src/lib.rs src/main.rs
git commit -m "chore: scaffold project with dependencies and config"
```

---

### Task 2: Config Module

**Files:**
- Create: `src/config.rs`

- [ ] **Step 1: Write `src/config.rs` with tests**

```rust
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
        Ok(config)
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
        let config = Config::load("config.toml").expect("should load config.toml");
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
        std::env::set_var("KIS_APP_KEY", "test_key");
        std::env::set_var("KIS_APP_SECRET", "test_secret");
        std::env::set_var("KIS_ACCOUNT_NO", "12345678");
        let creds = Credentials::from_env();
        assert_eq!(creds.app_key, "test_key");
        assert_eq!(creds.app_secret, "test_secret");
        assert_eq!(creds.account_no, "12345678");
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test config`
Expected: 2 tests PASS

- [ ] **Step 3: Commit**

```bash
git add src/config.rs
git commit -m "feat: add config loading from config.toml and env vars"
```

---

### Task 3: Logging Setup

**Files:**
- Create: `src/logging.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Write `src/logging.rs`**

```rust
use tracing_appender::rolling;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, fmt};

/// Initialise tracing to stdout and a daily rotating file under `logs/`.
/// Returns the file-appender guard — keep alive for the program's duration.
pub fn init() -> tracing_appender::non_blocking::WorkerGuard {
    std::fs::create_dir_all("logs").expect("failed to create logs/ directory");

    let file_appender = rolling::daily("logs", "protrader.log");
    let (non_blocking_file, guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_target(false)
                .with_level(false)
                .without_time()
                .with_ansi(false)
                .with_writer(non_blocking_file),
        )
        .with(
            fmt::layer()
                .with_target(false)
                .with_level(false)
                .without_time()
                .with_writer(std::io::stdout),
        )
        .init();

    guard
}
```

- [ ] **Step 2: Update `src/main.rs` to use logging**

```rust
fn main() {
    let _guard = protrader::logging::init();
    tracing::info!("ProTrader starting...");
}
```

- [ ] **Step 3: Run and verify output**

Run: `cargo run`
Expected: `ProTrader starting...` prints to stdout; `logs/protrader.YYYY-MM-DD.log` is created with the same line.

- [ ] **Step 4: Commit**

```bash
git add src/logging.rs src/main.rs
git commit -m "feat: add structured logging to stdout and daily rotating file"
```

---

### Task 4: Strategy Engine

**Files:**
- Create: `src/strategy.rs`

- [ ] **Step 1: Write `src/strategy.rs`**

```rust
use std::collections::HashMap;
use crate::config::{TradingConfig, RiskConfig};

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum ExitReason {
    StopLoss,
    DailyLimitReached,
    ForcedClose,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Signal {
    Buy { price: i64, qty: u32 },
    Exit { price: i64, reason: ExitReason },
    Hold,
}

#[derive(Debug, Clone)]
pub struct Position {
    pub entry_price: i64,
    pub qty: u32,
}

impl Position {
    pub fn unrealized_pnl(&self, current_price: i64) -> i64 {
        (current_price - self.entry_price) * self.qty as i64
    }

    pub fn realized_pnl(&self, exit_price: i64) -> i64 {
        (exit_price - self.entry_price) * self.qty as i64
    }
}

#[derive(Debug, Default, Clone)]
pub struct SessionPnl {
    pub realized: i64,
    pub unrealized: i64,
}

impl SessionPnl {
    pub fn total(&self) -> i64 {
        self.realized + self.unrealized
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionPhase {
    CapturingRange,
    Monitoring,
    Closed,
}

// ── Internal per-symbol state ─────────────────────────────────────────────────

#[derive(Debug)]
struct SymbolState {
    range_high: i64,       // 0 until first tick
    range_low: i64,        // i64::MAX until first tick
    position: Option<Position>,
    blacklisted: bool,
    unrealized_pnl: i64,   // last computed unrealized for this symbol
}

impl SymbolState {
    fn new() -> Self {
        Self {
            range_high: 0,
            range_low: i64::MAX,
            position: None,
            blacklisted: false,
            unrealized_pnl: 0,
        }
    }
}

// ── StrategyEngine ────────────────────────────────────────────────────────────

pub struct StrategyEngine {
    trading: TradingConfig,
    risk: RiskConfig,
    symbols: HashMap<String, SymbolState>,
    pub session_pnl: SessionPnl,
    phase: SessionPhase,
    daily_limit_hit: bool,
}

impl StrategyEngine {
    pub fn new(trading: TradingConfig, risk: RiskConfig, watchlist: &[String]) -> Self {
        let symbols = watchlist
            .iter()
            .map(|s| (s.clone(), SymbolState::new()))
            .collect();
        Self {
            trading,
            risk,
            symbols,
            session_pnl: SessionPnl::default(),
            phase: SessionPhase::CapturingRange,
            daily_limit_hit: false,
        }
    }

    pub fn set_phase(&mut self, phase: SessionPhase) {
        self.phase = phase;
    }

    /// Returns the qty of shares currently held for a symbol (0 if flat).
    pub fn get_position_qty(&self, symbol: &str) -> u32 {
        self.symbols
            .get(symbol)
            .and_then(|s| s.position.as_ref())
            .map(|p| p.qty)
            .unwrap_or(0)
    }

    /// Process a price tick for a symbol. Returns the signal to act on.
    pub fn on_tick(&mut self, symbol: &str, price: i64) -> Signal {
        // Step 1: update unrealized PnL for this symbol (ends borrow)
        if let Some(state) = self.symbols.get_mut(symbol) {
            if let Some(ref pos) = state.position {
                state.unrealized_pnl = pos.unrealized_pnl(price);
            }
        }
        // Step 2: recalculate total session unrealized (safe: no active borrow)
        self.session_pnl.unrealized = self.symbols.values().map(|s| s.unrealized_pnl).sum();

        let state = match self.symbols.get_mut(symbol) {
            Some(s) => s,
            None => return Signal::Hold,
        };

        match self.phase {
            SessionPhase::CapturingRange => {
                if price > state.range_high {
                    state.range_high = price;
                }
                if price < state.range_low {
                    state.range_low = price;
                }
                Signal::Hold
            }

            SessionPhase::Monitoring => {
                // Priority 1: stop-loss on open position (always checked)
                if let Some(ref pos) = state.position {
                    let stop_price =
                        (pos.entry_price as f64 * (1.0 - self.risk.stop_loss_pct / 100.0)) as i64;
                    if price <= stop_price {
                        return Signal::Exit {
                            price,
                            reason: ExitReason::StopLoss,
                        };
                    }
                }

                // Priority 2: no new entries if daily limit hit, position open, or blacklisted
                if self.daily_limit_hit || state.position.is_some() || state.blacklisted {
                    return Signal::Hold;
                }

                // Priority 3: check for breakout entry
                if state.range_high > 0 {
                    let breakout_price = (state.range_high as f64
                        * (1.0 + self.trading.breakout_buffer_pct / 100.0))
                        as i64;
                    if price > breakout_price {
                        let qty = (self.trading.fixed_amount_krw / price) as u32;
                        if qty > 0 {
                            return Signal::Buy { price, qty };
                        }
                    }
                }

                Signal::Hold
            }

            SessionPhase::Closed => {
                if state.position.is_some() {
                    return Signal::Exit {
                        price,
                        reason: ExitReason::ForcedClose,
                    };
                }
                Signal::Hold
            }
        }
    }

    /// Call after a buy order is confirmed to record the position.
    pub fn record_buy(&mut self, symbol: &str, price: i64, qty: u32) {
        if let Some(state) = self.symbols.get_mut(symbol) {
            state.position = Some(Position { entry_price: price, qty });
        }
    }

    /// Call after a sell order is confirmed. Records P&L, optionally blacklists.
    /// Returns realized P&L for this trade.
    pub fn record_exit(&mut self, symbol: &str, price: i64, blacklist: bool) -> i64 {
        let state = match self.symbols.get_mut(symbol) {
            Some(s) => s,
            None => return 0,
        };
        let pnl = state
            .position
            .take()
            .map(|p| p.realized_pnl(price))
            .unwrap_or(0);
        state.unrealized_pnl = 0;
        if blacklist {
            state.blacklisted = true;
        }
        self.session_pnl.realized += pnl;
        self.session_pnl.unrealized = self.symbols.values().map(|s| s.unrealized_pnl).sum();
        if self.session_pnl.realized < -self.risk.daily_loss_limit_krw {
            self.daily_limit_hit = true;
        }
        pnl
    }

    pub fn daily_limit_hit(&self) -> bool {
        self.daily_limit_hit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{TradingConfig, RiskConfig, TradingMode};

    fn make_engine() -> StrategyEngine {
        StrategyEngine::new(
            TradingConfig {
                mode: TradingMode::Paper,
                fixed_amount_krw: 500_000,
                breakout_buffer_pct: 0.2,
                range_minutes: 30,
                poll_interval_secs: 5,
                exit_time: "15:20".into(),
            },
            RiskConfig {
                stop_loss_pct: 1.5,
                daily_loss_limit_krw: 100_000,
            },
            &["005930".to_string()],
        )
    }

    #[test]
    fn test_range_capture_tracks_high_and_low() {
        let mut engine = make_engine();
        engine.on_tick("005930", 71_000);
        engine.on_tick("005930", 72_000);
        engine.on_tick("005930", 70_500);
        let state = engine.symbols.get("005930").unwrap();
        assert_eq!(state.range_high, 72_000);
        assert_eq!(state.range_low, 70_500);
    }

    #[test]
    fn test_no_buy_during_range_capture() {
        let mut engine = make_engine();
        let signal = engine.on_tick("005930", 99_000); // way above any breakout
        assert_eq!(signal, Signal::Hold);
        assert!(engine.symbols.get("005930").unwrap().position.is_none());
    }

    #[test]
    fn test_buy_signal_on_breakout() {
        let mut engine = make_engine();
        engine.on_tick("005930", 71_000); // sets range_high = 71_000
        engine.set_phase(SessionPhase::Monitoring);
        // breakout_price = 71_000 * 1.002 = 71_142
        // price 72_000 > 71_142 → Buy; qty = 500_000 / 72_000 = 6
        let signal = engine.on_tick("005930", 72_000);
        assert_eq!(signal, Signal::Buy { price: 72_000, qty: 6 });
    }

    #[test]
    fn test_no_buy_below_breakout_threshold() {
        let mut engine = make_engine();
        engine.on_tick("005930", 71_000);
        engine.set_phase(SessionPhase::Monitoring);
        // breakout_price = 71_142; price 71_100 < 71_142 → Hold
        let signal = engine.on_tick("005930", 71_100);
        assert_eq!(signal, Signal::Hold);
    }

    #[test]
    fn test_no_re_entry_while_position_open() {
        let mut engine = make_engine();
        engine.on_tick("005930", 71_000);
        engine.set_phase(SessionPhase::Monitoring);
        engine.record_buy("005930", 72_000, 6);
        // Already in a position — should not buy again
        let signal = engine.on_tick("005930", 75_000);
        assert_eq!(signal, Signal::Hold);
    }

    #[test]
    fn test_stop_loss_triggers_exit() {
        let mut engine = make_engine();
        engine.on_tick("005930", 71_000);
        engine.set_phase(SessionPhase::Monitoring);
        engine.record_buy("005930", 72_000, 6);
        // stop_loss at 72_000 * (1 - 0.015) = 70_920; price 70_800 <= 70_920
        let signal = engine.on_tick("005930", 70_800);
        assert_eq!(
            signal,
            Signal::Exit { price: 70_800, reason: ExitReason::StopLoss }
        );
    }

    #[test]
    fn test_record_exit_calculates_realized_pnl() {
        let mut engine = make_engine();
        engine.on_tick("005930", 71_000);
        engine.set_phase(SessionPhase::Monitoring);
        engine.record_buy("005930", 72_000, 6);
        let pnl = engine.record_exit("005930", 73_000, false);
        assert_eq!(pnl, (73_000 - 72_000) * 6); // +6_000
        assert_eq!(engine.session_pnl.realized, 6_000);
    }

    #[test]
    fn test_stop_loss_blacklists_symbol() {
        let mut engine = make_engine();
        engine.on_tick("005930", 71_000);
        engine.set_phase(SessionPhase::Monitoring);
        engine.record_buy("005930", 72_000, 6);
        engine.record_exit("005930", 70_800, true); // blacklist=true
        // No re-entry even on a clear breakout
        let signal = engine.on_tick("005930", 80_000);
        assert_eq!(signal, Signal::Hold);
        assert!(engine.symbols.get("005930").unwrap().blacklisted);
    }

    #[test]
    fn test_daily_loss_limit_stops_new_entries() {
        let mut engine = StrategyEngine::new(
            TradingConfig {
                mode: TradingMode::Paper,
                fixed_amount_krw: 500_000,
                breakout_buffer_pct: 0.2,
                range_minutes: 30,
                poll_interval_secs: 5,
                exit_time: "15:20".into(),
            },
            RiskConfig {
                stop_loss_pct: 1.5,
                daily_loss_limit_krw: 100_000,
            },
            &["005930".to_string(), "069500".to_string()],
        );
        engine.on_tick("005930", 71_000);
        engine.on_tick("069500", 9_000);
        engine.set_phase(SessionPhase::Monitoring);
        engine.record_buy("005930", 72_000, 6);
        // Loss of (55_000 - 72_000) * 6 = -102_000 → exceeds 100_000 limit
        engine.record_exit("005930", 55_000, false);
        assert!(engine.daily_limit_hit());
        // 069500 would break out but daily limit prevents entry
        let signal = engine.on_tick("069500", 9_500);
        assert_eq!(signal, Signal::Hold);
    }

    #[test]
    fn test_forced_close_on_closed_phase() {
        let mut engine = make_engine();
        engine.on_tick("005930", 71_000);
        engine.set_phase(SessionPhase::Monitoring);
        engine.record_buy("005930", 72_000, 6);
        engine.set_phase(SessionPhase::Closed);
        let signal = engine.on_tick("005930", 72_500);
        assert_eq!(
            signal,
            Signal::Exit { price: 72_500, reason: ExitReason::ForcedClose }
        );
    }

    #[test]
    fn test_get_position_qty() {
        let mut engine = make_engine();
        assert_eq!(engine.get_position_qty("005930"), 0);
        engine.record_buy("005930", 72_000, 6);
        assert_eq!(engine.get_position_qty("005930"), 6);
        engine.record_exit("005930", 73_000, false);
        assert_eq!(engine.get_position_qty("005930"), 0);
    }

    #[test]
    fn test_unrealized_pnl_updates_on_tick() {
        let mut engine = make_engine();
        engine.on_tick("005930", 71_000);
        engine.set_phase(SessionPhase::Monitoring);
        engine.record_buy("005930", 72_000, 6);
        engine.on_tick("005930", 73_000);
        // unrealized = (73_000 - 72_000) * 6 = 6_000
        assert_eq!(engine.session_pnl.unrealized, 6_000);
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test strategy`
Expected: all 11 tests PASS

- [ ] **Step 3: Commit**

```bash
git add src/strategy.rs
git commit -m "feat: add strategy engine with ORB logic, P&L tracking, stop-loss, and daily limit"
```

---

### Task 5: Order Client

**Files:**
- Create: `src/order.rs`

- [ ] **Step 1: Write `src/order.rs`**

```rust
use async_trait::async_trait;
use crate::config::Credentials;

#[derive(Debug, Clone)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone)]
pub struct OrderRequest {
    pub symbol: String,
    pub side: OrderSide,
    pub qty: u32,
    pub price: i64,
}

#[async_trait]
pub trait OrderClient: Send + Sync {
    async fn place_order(&self, req: &OrderRequest) -> anyhow::Result<()>;
}

// ── Paper ─────────────────────────────────────────────────────────────────────

pub struct PaperOrderClient;

#[async_trait]
impl OrderClient for PaperOrderClient {
    async fn place_order(&self, _req: &OrderRequest) -> anyhow::Result<()> {
        // Logging is handled by the scheduler; this is intentionally a no-op.
        Ok(())
    }
}

// ── Live (KIS) ────────────────────────────────────────────────────────────────

pub struct LiveOrderClient {
    http: reqwest::Client,
    base_url: String,
    credentials: Credentials,
    token: tokio::sync::RwLock<String>,
}

impl LiveOrderClient {
    pub async fn new(credentials: Credentials) -> anyhow::Result<Self> {
        let http = reqwest::Client::new();
        let base_url = "https://openapi.koreainvestment.com:9443".to_string();
        let token = Self::fetch_token(&http, &base_url, &credentials).await?;
        Ok(Self {
            http,
            base_url,
            credentials,
            token: tokio::sync::RwLock::new(token),
        })
    }

    async fn fetch_token(
        http: &reqwest::Client,
        base_url: &str,
        creds: &Credentials,
    ) -> anyhow::Result<String> {
        #[derive(serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
        }
        let resp: TokenResponse = http
            .post(format!("{}/oauth2/tokenP", base_url))
            .json(&serde_json::json!({
                "grant_type": "client_credentials",
                "appkey": creds.app_key,
                "appsecret": creds.app_secret,
            }))
            .send()
            .await?
            .json()
            .await?;
        Ok(resp.access_token)
    }
}

#[async_trait]
impl OrderClient for LiveOrderClient {
    async fn place_order(&self, req: &OrderRequest) -> anyhow::Result<()> {
        let token = self.token.read().await;
        let tr_id = match req.side {
            OrderSide::Buy => "TTTC0802U",
            OrderSide::Sell => "TTTC0801U",
        };
        let resp = self
            .http
            .post(format!(
                "{}/uapi/domestic-stock/v1/trading/order-cash",
                self.base_url
            ))
            .header("content-type", "application/json; charset=utf-8")
            .header("authorization", format!("Bearer {}", *token))
            .header("appkey", &self.credentials.app_key)
            .header("appsecret", &self.credentials.app_secret)
            .header("tr_id", tr_id)
            .json(&serde_json::json!({
                "CANO": self.credentials.account_no,
                "ACNT_PRDT_CD": "01",
                "PDNO": req.symbol,
                "ORD_DVSN": "00",
                "ORD_QTY": req.qty.to_string(),
                "ORD_UNPR": "0",
            }))
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("KIS order failed: HTTP {}", resp.status());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_paper_order_always_succeeds() {
        let client = PaperOrderClient;
        let req = OrderRequest {
            symbol: "005930".to_string(),
            side: OrderSide::Buy,
            qty: 7,
            price: 71_400,
        };
        client.place_order(&req).await.expect("paper order should not fail");
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test order`
Expected: 1 test PASS

- [ ] **Step 3: Commit**

```bash
git add src/order.rs
git commit -m "feat: add order client trait with paper and live KIS implementations"
```

---

### Task 6: Market Data Client

**Files:**
- Create: `src/market_data.rs`

- [ ] **Step 1: Write `src/market_data.rs`**

```rust
use async_trait::async_trait;
use crate::config::Credentials;

#[async_trait]
pub trait MarketDataClient: Send + Sync {
    async fn fetch_price(&self, symbol: &str) -> anyhow::Result<i64>;
}

// ── KIS HTTP implementation ───────────────────────────────────────────────────

pub struct KisMarketDataClient {
    http: reqwest::Client,
    base_url: String,
    credentials: Credentials,
    token: tokio::sync::RwLock<String>,
}

impl KisMarketDataClient {
    pub async fn new(credentials: Credentials) -> anyhow::Result<Self> {
        let http = reqwest::Client::new();
        let base_url = "https://openapi.koreainvestment.com:9443".to_string();
        let token = Self::fetch_token(&http, &base_url, &credentials).await?;
        Ok(Self {
            http,
            base_url,
            credentials,
            token: tokio::sync::RwLock::new(token),
        })
    }

    async fn fetch_token(
        http: &reqwest::Client,
        base_url: &str,
        creds: &Credentials,
    ) -> anyhow::Result<String> {
        #[derive(serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
        }
        let resp: TokenResponse = http
            .post(format!("{}/oauth2/tokenP", base_url))
            .json(&serde_json::json!({
                "grant_type": "client_credentials",
                "appkey": creds.app_key,
                "appsecret": creds.app_secret,
            }))
            .send()
            .await?
            .json()
            .await?;
        Ok(resp.access_token)
    }
}

#[async_trait]
impl MarketDataClient for KisMarketDataClient {
    async fn fetch_price(&self, symbol: &str) -> anyhow::Result<i64> {
        #[derive(serde::Deserialize)]
        struct PriceOutput {
            stck_prpr: String, // current price as a string in KIS API
        }
        #[derive(serde::Deserialize)]
        struct PriceResponse {
            output: PriceOutput,
        }

        let token = self.token.read().await;
        let resp: PriceResponse = self
            .http
            .get(format!(
                "{}/uapi/domestic-stock/v1/quotations/inquire-price",
                self.base_url
            ))
            .header("content-type", "application/json; charset=utf-8")
            .header("authorization", format!("Bearer {}", *token))
            .header("appkey", &self.credentials.app_key)
            .header("appsecret", &self.credentials.app_secret)
            .header("tr_id", "FHKST01010100")
            .query(&[
                ("FID_COND_MRKT_DIV_CODE", "J"),
                ("FID_INPUT_ISCD", symbol),
            ])
            .send()
            .await?
            .json()
            .await?;

        let price: i64 = resp.output.stck_prpr.trim().parse()?;
        Ok(price)
    }
}

// ── Mock for testing ──────────────────────────────────────────────────────────

/// Returns prices from a pre-loaded sequence per symbol.
/// Repeats the last price once the sequence is exhausted.
pub struct MockMarketDataClient {
    prices: std::collections::HashMap<
        String,
        std::sync::Mutex<std::collections::VecDeque<i64>>,
    >,
}

impl MockMarketDataClient {
    pub fn new(prices: std::collections::HashMap<String, Vec<i64>>) -> Self {
        Self {
            prices: prices
                .into_iter()
                .map(|(k, v)| (k, std::sync::Mutex::new(v.into())))
                .collect(),
        }
    }
}

#[async_trait]
impl MarketDataClient for MockMarketDataClient {
    async fn fetch_price(&self, symbol: &str) -> anyhow::Result<i64> {
        let mut deque = self
            .prices
            .get(symbol)
            .ok_or_else(|| anyhow::anyhow!("unknown symbol: {}", symbol))?
            .lock()
            .unwrap();
        if deque.len() > 1 {
            Ok(deque.pop_front().unwrap())
        } else {
            Ok(*deque.front().unwrap())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_returns_sequence_then_repeats_last() {
        let mut prices = std::collections::HashMap::new();
        prices.insert("005930".to_string(), vec![71_000, 72_000, 73_000]);
        let client = MockMarketDataClient::new(prices);

        assert_eq!(client.fetch_price("005930").await.unwrap(), 71_000);
        assert_eq!(client.fetch_price("005930").await.unwrap(), 72_000);
        assert_eq!(client.fetch_price("005930").await.unwrap(), 73_000);
        assert_eq!(client.fetch_price("005930").await.unwrap(), 73_000); // repeated
    }

    #[tokio::test]
    async fn test_mock_unknown_symbol_returns_error() {
        let client = MockMarketDataClient::new(std::collections::HashMap::new());
        assert!(client.fetch_price("unknown").await.is_err());
    }
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test market_data`
Expected: 2 tests PASS

- [ ] **Step 3: Commit**

```bash
git add src/market_data.rs
git commit -m "feat: add market data client with KIS HTTP implementation and mock for tests"
```

---

### Task 7: Scheduler

**Files:**
- Create: `src/scheduler.rs`

- [ ] **Step 1: Write `src/scheduler.rs`**

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use chrono::Timelike;
use chrono_tz::Asia::Seoul;

use crate::config::{Config, TradingMode};
use crate::strategy::{StrategyEngine, SessionPhase, Signal, ExitReason};
use crate::order::{OrderClient, OrderRequest, OrderSide};
use crate::market_data::MarketDataClient;

pub struct SessionScheduler {
    config: Arc<Config>,
    engine: Arc<Mutex<StrategyEngine>>,
    market_data: Arc<dyn MarketDataClient>,
    order_client: Arc<dyn OrderClient>,
}

impl SessionScheduler {
    pub fn new(
        config: Arc<Config>,
        engine: Arc<Mutex<StrategyEngine>>,
        market_data: Arc<dyn MarketDataClient>,
        order_client: Arc<dyn OrderClient>,
    ) -> Self {
        Self { config, engine, market_data, order_client }
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        let mode_str = match self.config.trading.mode {
            TradingMode::Paper => "PAPER",
            TradingMode::Live => "LIVE",
        };

        // ── Wait until 09:00 KST ────────────────────────────────────────────
        self.wait_until(9, 0).await;
        tracing::info!("Session started — capturing opening range until 09:{:02}", self.config.trading.range_minutes);

        let shutdown = Arc::new(AtomicBool::new(false));

        // ── Spawn one task per symbol ────────────────────────────────────────
        let handles: Vec<_> = self
            .config
            .symbols
            .watchlist
            .iter()
            .map(|symbol| {
                let symbol = symbol.clone();
                let engine = self.engine.clone();
                let market_data = self.market_data.clone();
                let order_client = self.order_client.clone();
                let poll = Duration::from_secs(self.config.trading.poll_interval_secs);
                let mode = mode_str.to_string();
                let shutdown = shutdown.clone();
                tokio::spawn(async move {
                    symbol_loop(symbol, engine, market_data, order_client, poll, mode, shutdown).await;
                })
            })
            .collect();

        // ── Wait until end of range window → switch to Monitoring ───────────
        self.wait_until(9, self.config.trading.range_minutes).await;
        self.engine.lock().await.set_phase(SessionPhase::Monitoring);
        tracing::info!("Opening range locked — monitoring for breakouts");

        // ── Wait until exit_time → switch to Closed ─────────────────────────
        let (exit_h, exit_m) = parse_time(&self.config.trading.exit_time);
        self.wait_until(exit_h, exit_m).await;
        self.engine.lock().await.set_phase(SessionPhase::Closed);
        tracing::info!("Exit time reached — forcing close of all open positions");

        // Give symbol tasks one extra poll cycle to process the ForcedClose signals
        sleep(Duration::from_secs(self.config.trading.poll_interval_secs + 2)).await;

        // Signal all tasks to stop
        shutdown.store(true, Ordering::Relaxed);
        for h in handles {
            let _ = h.await;
        }

        let pnl = self.engine.lock().await.session_pnl.clone();
        tracing::info!(
            "Session complete | realized={} | unrealized={} | total={}",
            pnl.realized,
            pnl.unrealized,
            pnl.total()
        );

        Ok(())
    }

    /// Sleep until a given HH:MM KST wall-clock time. Returns immediately if already past.
    async fn wait_until(&self, hour: u32, minute: u32) {
        loop {
            let now = chrono::Utc::now().with_timezone(&Seoul);
            let now_min = now.hour() * 60 + now.minute();
            let target_min = hour * 60 + minute;
            if now_min >= target_min {
                return;
            }
            let secs_remaining = (target_min - now_min) * 60 - now.second();
            tracing::info!("Waiting {}s until {:02}:{:02} KST", secs_remaining, hour, minute);
            sleep(Duration::from_secs(secs_remaining as u64)).await;
        }
    }
}

async fn symbol_loop(
    symbol: String,
    engine: Arc<Mutex<StrategyEngine>>,
    market_data: Arc<dyn MarketDataClient>,
    order_client: Arc<dyn OrderClient>,
    poll: Duration,
    mode_str: String,
    shutdown: Arc<AtomicBool>,
) {
    let mut retry_count = 0u32;

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        sleep(poll).await;

        let price = match market_data.fetch_price(&symbol).await {
            Ok(p) => {
                retry_count = 0;
                p
            }
            Err(e) => {
                retry_count += 1;
                tracing::warn!(
                    "symbol={} price fetch error (attempt {}): {}",
                    symbol, retry_count, e
                );
                if retry_count >= 3 {
                    tracing::warn!("symbol={} skipping tick after 3 failures", symbol);
                    retry_count = 0;
                }
                continue;
            }
        };

        let signal = engine.lock().await.on_tick(&symbol, price);

        match signal {
            Signal::Buy { price, qty } => {
                let amount = price * qty as i64;
                let ts = now_kst();
                tracing::info!(
                    "{} KST | [{}] | BUY {} | price={} | qty={} | amount={}",
                    ts, mode_str, symbol, price, qty, amount
                );
                let req = OrderRequest { symbol: symbol.clone(), side: OrderSide::Buy, qty, price };
                match order_client.place_order(&req).await {
                    Ok(_) => {
                        engine.lock().await.record_buy(&symbol, price, qty);
                    }
                    Err(e) => {
                        tracing::error!(
                            "{} KST | [{}] | ORDER FAILED {} | error={}",
                            now_kst(), mode_str, symbol, e
                        );
                    }
                }
            }

            Signal::Exit { price, reason } => {
                let blacklist = matches!(reason, ExitReason::StopLoss);
                let qty = engine.lock().await.get_position_qty(&symbol);
                let pnl = engine.lock().await.record_exit(&symbol, price, blacklist);
                let reason_str = match reason {
                    ExitReason::StopLoss => "STOP-LOSS",
                    ExitReason::DailyLimitReached => "DAILY-LIMIT",
                    ExitReason::ForcedClose => "FORCED-CLOSE",
                };
                let blacklist_suffix = if blacklist { " | blacklisted" } else { "" };
                tracing::info!(
                    "{} KST | [{}] | {} {} | price={} | realized_pnl={}{}",
                    now_kst(), mode_str, reason_str, symbol, price, pnl, blacklist_suffix
                );
                let req = OrderRequest { symbol: symbol.clone(), side: OrderSide::Sell, qty, price };
                if let Err(e) = order_client.place_order(&req).await {
                    tracing::error!(
                        "{} KST | [{}] | ORDER FAILED {} | error={}",
                        now_kst(), mode_str, symbol, e
                    );
                }
                let session = engine.lock().await.session_pnl.clone();
                tracing::info!(
                    "{} KST | [{}] | SESSION PnL | realized={} | unrealized={} | total={}",
                    now_kst(), mode_str, session.realized, session.unrealized, session.total()
                );
            }

            Signal::Hold => {}
        }
    }
}

fn now_kst() -> String {
    chrono::Utc::now()
        .with_timezone(&Seoul)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

pub fn parse_time(s: &str) -> (u32, u32) {
    let parts: Vec<&str> = s.splitn(2, ':').collect();
    let h: u32 = parts[0].parse().expect("invalid exit_time hour");
    let m: u32 = parts[1].parse().expect("invalid exit_time minute");
    (h, m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_time_standard() {
        assert_eq!(parse_time("15:20"), (15, 20));
    }

    #[test]
    fn test_parse_time_zero_padded() {
        assert_eq!(parse_time("09:00"), (9, 0));
    }
}
```

- [ ] **Step 2: Run tests and verify compile**

Run: `cargo test scheduler && cargo build`
Expected: 2 tests PASS; binary compiles

- [ ] **Step 3: Commit**

```bash
git add src/scheduler.rs
git commit -m "feat: add session scheduler with phase transitions and per-symbol async task loop"
```

---

### Task 8: Main Entry Point

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Write full `src/main.rs`**

```rust
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
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: compiles with no errors

- [ ] **Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat: wire up main entry point — loads config, credentials, and runs scheduler"
```

---

### Task 9: Integration Tests

**Files:**
- Create: `tests/integration_test.rs`

- [ ] **Step 1: Write `tests/integration_test.rs`**

```rust
use std::sync::Arc;
use std::collections::HashMap;
use tokio::sync::Mutex;

use protrader::config::{Config, TradingConfig, RiskConfig, SymbolsConfig, TradingMode};
use protrader::strategy::{StrategyEngine, SessionPhase, Signal, ExitReason};
use protrader::market_data::MockMarketDataClient;
use protrader::order::PaperOrderClient;

fn paper_config(watchlist: Vec<String>) -> Config {
    Config {
        trading: TradingConfig {
            mode: TradingMode::Paper,
            fixed_amount_krw: 500_000,
            breakout_buffer_pct: 0.2,
            range_minutes: 30,
            poll_interval_secs: 0,
            exit_time: "15:20".to_string(),
        },
        risk: RiskConfig {
            stop_loss_pct: 1.5,
            daily_loss_limit_krw: 100_000,
        },
        symbols: SymbolsConfig { watchlist },
    }
}

/// Full session: range capture → breakout buy → forced close at end of day.
/// Prices: 71_000, 71_500, 72_000 (range), 72_100 (below breakout), 73_000 (buy), 73_500 (exit)
#[tokio::test]
async fn test_full_session_buy_and_forced_close() {
    let config = paper_config(vec!["005930".to_string()]);
    let engine = Arc::new(Mutex::new(StrategyEngine::new(
        config.trading.clone(),
        config.risk.clone(),
        &config.symbols.watchlist,
    )));

    let mut price_map = HashMap::new();
    price_map.insert(
        "005930".to_string(),
        vec![71_000i64, 71_500, 72_000, 72_100, 73_000, 73_500],
    );
    let market_data = Arc::new(MockMarketDataClient::new(price_map));

    // ── Range capture: 3 ticks ───────────────────────────────────────────────
    for _ in 0..3 {
        let p = market_data.fetch_price("005930").await.unwrap();
        engine.lock().await.on_tick("005930", p);
    }

    // ── Switch to Monitoring ─────────────────────────────────────────────────
    engine.lock().await.set_phase(SessionPhase::Monitoring);

    // Tick at 72_100 — below breakout (72_000 * 1.002 = 72_144) → Hold
    let p = market_data.fetch_price("005930").await.unwrap();
    let signal = engine.lock().await.on_tick("005930", p);
    assert_eq!(signal, Signal::Hold);

    // Tick at 73_000 — above breakout → Buy; qty = 500_000 / 73_000 = 6
    let p = market_data.fetch_price("005930").await.unwrap();
    let signal = engine.lock().await.on_tick("005930", p);
    assert_eq!(signal, Signal::Buy { price: 73_000, qty: 6 });
    engine.lock().await.record_buy("005930", 73_000, 6);

    // ── Switch to Closed (end of day) ────────────────────────────────────────
    engine.lock().await.set_phase(SessionPhase::Closed);

    // Tick at 73_500 → ForcedClose
    let p = market_data.fetch_price("005930").await.unwrap();
    let signal = engine.lock().await.on_tick("005930", p);
    assert_eq!(signal, Signal::Exit { price: 73_500, reason: ExitReason::ForcedClose });

    let pnl = engine.lock().await.record_exit("005930", 73_500, false);
    assert_eq!(pnl, (73_500 - 73_000) * 6); // +3_000

    let session = engine.lock().await.session_pnl.clone();
    assert_eq!(session.realized, 3_000);
    assert_eq!(session.total(), 3_000);
}

/// Stop-loss fires, symbol is blacklisted, no re-entry even on subsequent breakout.
#[tokio::test]
async fn test_stop_loss_blacklists_and_prevents_reentry() {
    let config = paper_config(vec!["005930".to_string()]);
    let engine = Arc::new(Mutex::new(StrategyEngine::new(
        config.trading.clone(),
        config.risk.clone(),
        &config.symbols.watchlist,
    )));

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

    let session = engine.lock().await.session_pnl.clone();
    assert_eq!(session.realized, -7_200);
}

/// After daily loss limit is hit, no new buy entries are allowed on any symbol.
#[tokio::test]
async fn test_daily_loss_limit_stops_all_new_entries() {
    let config = paper_config(vec!["005930".to_string(), "069500".to_string()]);
    let engine = Arc::new(Mutex::new(StrategyEngine::new(
        config.trading.clone(),
        config.risk.clone(),
        &config.symbols.watchlist,
    )));

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
    // but daily limit is hit → Hold
    let signal = engine.lock().await.on_tick("069500", 9_500);
    assert_eq!(signal, Signal::Hold);
}

/// P&L tracking: unrealized updates on tick, realized on close.
#[tokio::test]
async fn test_pnl_tracking_unrealized_and_realized() {
    let config = paper_config(vec!["005930".to_string()]);
    let engine = Arc::new(Mutex::new(StrategyEngine::new(
        config.trading.clone(),
        config.risk.clone(),
        &config.symbols.watchlist,
    )));

    engine.lock().await.on_tick("005930", 71_000);
    engine.lock().await.set_phase(SessionPhase::Monitoring);
    engine.lock().await.record_buy("005930", 72_000, 6);

    // Price tick at 73_000: unrealized = (73_000 - 72_000) * 6 = 6_000
    engine.lock().await.on_tick("005930", 73_000);
    let session = engine.lock().await.session_pnl.clone();
    assert_eq!(session.unrealized, 6_000);
    assert_eq!(session.realized, 0);

    // Close at 73_500: realized = (73_500 - 72_000) * 6 = 9_000
    engine.lock().await.record_exit("005930", 73_500, false);
    let session = engine.lock().await.session_pnl.clone();
    assert_eq!(session.realized, 9_000);
    assert_eq!(session.unrealized, 0);
    assert_eq!(session.total(), 9_000);
}
```

- [ ] **Step 2: Run integration tests**

Run: `cargo test --test integration_test`
Expected: all 4 tests PASS

- [ ] **Step 3: Run the full test suite**

Run: `cargo test`
Expected: all tests PASS (unit + integration)

- [ ] **Step 4: Commit**

```bash
git add tests/integration_test.rs
git commit -m "test: add integration tests for full session, stop-loss, daily limit, and P&L"
```

---

## Notes for Live Mode

- Credentials via `.env` (copy from `.env.example`, never commit `.env`)
- `KIS_APP_KEY`, `KIS_APP_SECRET`, `KIS_ACCOUNT_NO` must be set
- KIS tokens expire; when yours is renewed, `KisMarketDataClient::new` and `LiveOrderClient::new` will fetch a fresh token at startup
- Switch `mode = "live"` in `config.toml` only after validating paper mode behavior
- KIS virtual trading URL (`https://openapivts.koreainvestment.com:9443`) can be used for manual testing once credentials are renewed
