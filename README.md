# protrader

An async Rust trading bot for the Korean stock market (KOSPI) via the [Korea Investment & Securities (KIS) Open API](https://apiportal.koreainvestment.com). Supports multiple intraday strategies that can be run live or compared side-by-side in backtesting.

## Strategies

All strategies share a common `Strategy` trait and respect the same session lifecycle, stop-loss, and daily loss limit rules.

| Strategy | Entry | Exit |
|---|---|---|
| **ORB** (Opening Range Breakout) | Price breaks above range high + buffer | Stop-loss, daily limit, or EOD |
| **EMA Crossover** | Fast EMA crosses above slow EMA (golden cross) | Death cross, stop-loss, daily limit, or EOD |
| **VWAP Mean Reversion** | Price drops below running VWAP by a threshold | Price reverts to VWAP, stop-loss, daily limit, or EOD |

**Session lifecycle (KST)**

```
09:00 → CapturingRange  — builds indicators (ORB range, EMAs, VWAP)
09:30 → Monitoring      — watches for entries and exits
16:00 → Closed          — forces exit of all open positions
```

## Setup

**Prerequisites:** Rust toolchain, a KIS Open API account.

1. Clone the repo and copy the example env file:

```bash
cp .env.example .env
```

2. Fill in your KIS credentials in `.env`:

```
KIS_APP_KEY=your_app_key_here
KIS_APP_SECRET=your_app_secret_here
KIS_ACCOUNT_NO=your_account_number_here
```

3. (Optional) Add Telegram credentials for trade notifications:

```
TELEGRAM_BOT_TOKEN=your_bot_token_here
TELEGRAM_CHAT_ID=your_chat_id_here
```

4. Edit `config.toml` to configure your watchlist and parameters (see [Configuration](#configuration)).

## Running

```bash
# Paper trading (no real orders)
cargo run

# Release build
cargo build --release && ./target/release/protrader
```

Logs are written to stdout and to `logs/protrader.log.YYYY-MM-DD` (daily rotation).

## Backtesting

Fetch historical minute bars from KIS and run all strategy variants over the same tick stream:

```bash
# Single day
cargo run --bin backtest -- 2025-01-15

# Date range (data cached under data/YYYYMMDD/<symbol>.csv)
cargo run --bin backtest -- 2025-01-01 2025-01-31
```

Example output:

```
Strategy                   Realized P&L      Total P&L   Trades
─────────────────────────────────────────────────────────────────
ORB-default                      +₩12400        +₩12400        4
ORB-tight-buffer                  +₩8200         +₩8200        6
ORB-wide-stoploss                +₩15600        +₩15600        4
EMA-cross-5-20                   +₩21000        +₩21000       12
EMA-cross-3-10                    -₩4200         -₩4200       18
VWAP-reversion-1pct               +₩9800         +₩9800        8
VWAP-reversion-0.5pct            +₩18200        +₩18200       14
```

## Configuration

`config.toml`:

```toml
[trading]
mode = "paper"           # "paper" or "live"
fixed_amount = 500000    # per-trade budget in KRW
breakout_buffer_pct = 0.2  # % above range high to trigger ORB buy
range_minutes = 30       # opening range duration (minutes after open_time)
poll_interval_secs = 5   # price polling frequency

[risk]
stop_loss_pct = 5        # stop-loss trigger (% below entry price)
daily_loss_limit = 100000  # max realized loss before halting new entries (KRW)

[market]
timezone = "Asia/Seoul"
open_time = "09:00"
exit_time = "16:00"      # forced close time

[[symbols]]
ticker = "005930"        # Samsung Electronics

[[symbols]]
ticker = "069500"        # KODEX 200 ETF
```

Per-symbol overrides for `fixed_amount`, `breakout_buffer_pct`, and `stop_loss_pct` are supported:

```toml
[[symbols]]
ticker = "005930"
fixed_amount = 1000000
stop_loss_pct = 3.0
```

## Paper trading

In `paper` mode no real orders are placed. `PaperOrderClient` maintains a simulated account (default 10,000,000 KRW starting balance) and logs account state after every order:

```
[PAPER] BUY 005930 qty=7 price=71400 cost=499800 | balance=9500200 shares=7 avg_cost=71400
[PAPER] SELL 005930 qty=7 price=74000 proceeds=518000 pnl=18200 | balance=10018200 realized_pnl=18200 return=0.18% unrealized_equity=0
```

## Development

```bash
cargo check                          # type-check without building
cargo test                           # unit + integration tests
cargo test test_buy_signal_on_breakout  # run a single test
cargo test --lib strategy::tests     # tests for a specific module
cargo test --test integration_test   # integration tests only
cargo test live_price -- --ignored --nocapture  # live KIS API smoke test (requires .env)
cargo clippy                         # lint
```

## Architecture

| Module | Responsibility |
|---|---|
| `src/strategy.rs` | Pure synchronous signal logic; no I/O. Defines the `Strategy` trait and `OrbStrategy`, `EmaCrossStrategy`, `VwapReversionStrategy`. `StrategyEngine` wraps any strategy and enforces daily loss limits and phase transitions. |
| `src/scheduler.rs` | Async orchestration. Drives session phases, spawns per-symbol Tokio tasks, places orders. |
| `src/backtest.rs` | `BacktestRunner` replays historical ticks through multiple engines simultaneously. Supports single-day and multi-day modes with per-day resets. |
| `src/historical.rs` | Fetches and caches minute-bar ticks from the KIS API. |
| `src/market_data.rs` | `MarketDataClient` trait — `KisMarketDataClient` (live HTTP) and `MockMarketDataClient` (test replay). |
| `src/order.rs` | `OrderClient` trait — `PaperOrderClient` (simulated account) and `LiveOrderClient` (KIS HTTP). |
| `src/auth.rs` | KIS OAuth token management. |
| `src/config.rs` | `config.toml` deserialization and validation. |
| `src/logging.rs` | Dual stdout + daily-rotating file logging. |
| `src/telegram.rs` | Optional Telegram notifications on buy/sell. |

**Key design decisions:**
- Prices are `i64` (Korean Won, integer) — no floating point for money.
- Daily loss limit is checked against **realized P&L only**, not unrealized, to avoid halting on temporary dips.
- Stop-loss blacklists the symbol for the rest of the session — no re-entry.
- `StrategyEngine` is I/O-free; all async work lives in `scheduler.rs`.
- `ExitReason` distinguishes `StopLoss` (blacklists), `DailyLimitReached`, `ForcedClose` (EOD), and `SignalExit` (strategy-driven: EMA death cross, VWAP reversion).
