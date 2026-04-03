# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Build
cargo build
cargo build --release

# Run (requires config.toml and .env with KIS credentials)
cargo run

# Test (unit + integration, excluding live API tests)
cargo test

# Run a single test by name
cargo test test_buy_signal_on_breakout

# Run only unit tests for a specific module
cargo test --lib strategies::tests

# Run live KIS API smoke test (requires real credentials in .env)
cargo test live_price -- --ignored --nocapture

# Check without building
cargo check

# Lint
cargo clippy

# Backtest (requires .env with KIS credentials; caches data under data/)
cargo run --bin backtest -- <YYYY-MM-DD>                   # single day
cargo run --bin backtest -- <YYYY-MM-DD> <YYYY-MM-DD>     # date range
```

## Architecture

This is an async Rust trading bot targeting the Korean stock market (KOSPI) via the Korea Investment & Securities (KIS) Open API. It supports multiple intraday trading strategies that share a common `Strategy` trait and are compared side-by-side in the backtesting binary.

### Session lifecycle (all times KST)

```
09:00 → CapturingRange phase: builds indicators (range high/low, EMAs, VWAP)
09:30 → Monitoring phase: watches for entries and exits
15:20 → Closed phase: forces exit of all open positions
```

`SessionScheduler` (`src/scheduler.rs`) drives the lifecycle using `wait_until()` to sleep until KST wall-clock times. It spawns one `symbol_loop` task per symbol (Tokio), each polling price at `poll_interval_secs` and delegating signals to `StrategyEngine`.

**Mid-session startup:** if the bot starts after the `CapturingRange` window (09:30), it fetches today's historical minute bars via `KisHistoricalClient` and replays them through the engine before spawning symbol tasks, so all strategies have warm-up history (ORB range, EMA seeds, VWAP accumulation) before monitoring begins.

### Core modules

- **`src/strategies/`** — Pure synchronous logic, no I/O. Defines the `Strategy` trait and three implementations:
  - `OrbStrategy` — Opening Range Breakout: captures high/low during `CapturingRange`, enters on a configurable breakout above range high, exits on stop-loss or EOD.
  - `EmaCrossStrategy` — EMA crossover: enters long on a golden cross (fast EMA > slow EMA), exits on a death cross or stop-loss. Periods are configurable. EMAs accumulate during `CapturingRange` for warm-up.
  - `VwapReversionStrategy` — Mean reversion: enters when price drops more than a threshold below the running session VWAP proxy (equal-weighted price average), exits when price reverts to VWAP. VWAP accumulates during `CapturingRange`.
  - `StrategyEngine` wraps any `Strategy` and manages phase transitions and the daily loss limit. Signal priority in `Monitoring` phase for all strategies: stop-loss > daily limit > strategy-specific exit > strategy-specific entry.

- **`src/scheduler.rs`** — Async orchestration. Wraps `StrategyEngine` in `Arc<Mutex<>>` shared across symbol tasks. Handles order placement, logging, and P&L recording. The split between `on_tick()` (signal) and `record_buy/record_exit()` (state mutation) is intentional: state is only updated after order confirmation, not before. Performs mid-session range reconstruction before spawning symbol tasks.

- **`src/backtest.rs`** — `BacktestRunner` drives multiple `StrategyEngine` instances over the same historical tick stream. Supports single-day (`run`) and multi-day (`run_days`) modes; engines are reset between days so range, positions, blacklists, and the daily loss limit all start fresh each session. P&L accumulates across days.

- **`src/historical.rs`** — `KisHistoricalClient` fetches minute-bar ticks from the KIS API. Results are cached under `data/YYYYMMDD/<symbol>.csv` to avoid redundant API calls on repeated backtest runs.

- **`src/market_data.rs`** — `MarketDataClient` trait with `KisMarketDataClient` (live HTTP) and `MockMarketDataClient` (test sequences). `MockMarketDataClient` replays a `VecDeque` of prices per symbol, repeating the last value when exhausted.

- **`src/order.rs`** — `OrderClient` trait with `PaperOrderClient` (simulated account) and `LiveOrderClient` (KIS HTTP). KIS tr_ids: `TTTC0802U` (buy), `TTTC0801U` (sell).

- **`src/auth.rs`** — `KisAuthProvider` fetches and caches OAuth tokens from the KIS API.

- **`src/config.rs`** — Loads `config.toml` via TOML deserialization. `KisCredentials` reads `KIS_APP_KEY`, `KIS_APP_SECRET`, `KIS_ACCOUNT_NO` from environment (panics if missing). Config is loaded before logging is initialised so `[logging].level` takes effect from the first log line.

- **`src/logging.rs`** — Dual output: stdout and daily-rotating file under `logs/protrader.log.YYYY-MM-DD`. Log level is controlled by `[logging].level` in `config.toml` (default: `"info"`).

- **`src/lib.rs`** — Exports `http_client()`: a shared `reqwest::Client` with `pool_idle_timeout=25s` and `tcp_keepalive=15s`. All HTTP clients in the codebase use this to avoid "connection closed before message completed" errors from the KIS API.

### Key design decisions

- Prices are `i64` (Korean Won, integer). No floating point for money.
- Daily loss limit is checked against **realized P&L only** (not unrealized) to avoid halting on temporary dips.
- Stop-loss blacklists the symbol for the session — no re-entry after a stop-loss exit.
- `StrategyEngine` is intentionally I/O-free; all async work lives in `scheduler.rs`.
- `ExitReason` has four variants: `StopLoss` (blacklists symbol), `DailyLimitReached`, `ForcedClose` (EOD), `SignalExit` (strategy-driven: EMA death cross, VWAP reversion complete).
- All HTTP clients are built via `crate::http_client()` — never `reqwest::Client::new()` directly.

### Backtest binary (`src/bin/backtest.rs`)

Runs 7 strategy variants over the same tick stream for direct comparison:

| Variant | Strategy |
|---|---|
| `ORB-default` | ORB with config defaults |
| `ORB-tight-buffer` | ORB, 0.1% breakout buffer |
| `ORB-wide-stoploss` | ORB, 3% stop-loss |
| `EMA-cross-5-20` | EMA crossover, fast=5 / slow=20 |
| `EMA-cross-3-10` | EMA crossover, fast=3 / slow=10 |
| `VWAP-reversion-1pct` | VWAP mean reversion, 1% dip threshold |
| `VWAP-reversion-0.5pct` | VWAP mean reversion, 0.5% dip threshold |

### Configuration (`config.toml`)

| Key | Description |
|-----|-------------|
| `trading.mode` | `"paper"` or `"live"` |
| `trading.fixed_amount` | Per-trade budget in KRW |
| `trading.breakout_buffer_pct` | % above range high to trigger ORB buy |
| `trading.range_minutes` | Duration of opening range capture (minutes after 09:00) |
| `trading.poll_interval_secs` | Price polling frequency |
| `risk.stop_loss_pct` | Stop-loss trigger (% below entry) |
| `risk.daily_loss_limit` | Max realized loss per session before halting new entries |
| `strategy.type` | `"orb"`, `"ema_cross"`, or `"vwap_reversion"` |
| `logging.level` | Log verbosity: `"error"`, `"warn"`, `"info"`, `"debug"`, `"trace"` (default: `"info"`) |
| `market.timezone` | IANA timezone string (e.g. `"Asia/Seoul"`) |
| `market.open_time` | Session open time in `HH:MM` |
| `market.exit_time` | Forced close time in `HH:MM` |
| `[[symbols]]` | KOSPI ticker codes; each entry may override `fixed_amount`, `breakout_buffer_pct`, `stop_loss_pct` |

### Credentials (`.env`)

Copy `.env.example` to `.env` and set `KIS_APP_KEY`, `KIS_APP_SECRET`, `KIS_ACCOUNT_NO`. The live smoke test in `market_data.rs` is gated with `#[ignore]` and requires real credentials.
