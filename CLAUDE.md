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
cargo test --lib strategy::tests

# Run only integration tests
cargo test --test integration_test

# Run live KIS API smoke test (requires real credentials in .env)
cargo test live_price -- --ignored --nocapture

# Check without building
cargo check

# Lint
cargo clippy
```

## Architecture

This is an async Rust trading bot targeting the Korean stock market (KOSPI) via the Korea Investment & Securities (KIS) Open API. It implements an **Opening Range Breakout (ORB)** strategy.

### Session lifecycle (all times KST)

```
09:00 → CapturingRange phase: tracks high/low for each symbol
09:30 → Monitoring phase: watches for breakout entries, stop-losses
15:20 → Closed phase: forces exit of all open positions
```

`SessionScheduler` (`src/scheduler.rs`) drives the lifecycle using `wait_until()` to sleep until KST wall-clock times. It spawns one `symbol_loop` task per symbol (Tokio), each polling price at `poll_interval_secs` and delegating signals to `StrategyEngine`.

### Core modules

- **`src/strategy.rs`** — Pure synchronous logic, no I/O. `StrategyEngine` holds per-symbol state (`SymbolState`: range high/low, position, blacklist flag) and session-level P&L. `on_tick()` is the main signal emitter; `record_buy()`/`record_exit()` mutate state after orders are confirmed. Signal priority in `Monitoring` phase: stop-loss > daily limit > no-reentry guard > breakout entry.

- **`src/scheduler.rs`** — Async orchestration. Wraps `StrategyEngine` in `Arc<Mutex<>>` shared across symbol tasks. Handles order placement, logging, and P&L recording. The split between `on_tick()` (signal) and `record_buy/record_exit()` (state mutation) is intentional: state is only updated after order confirmation, not before.

- **`src/market_data.rs`** — `MarketDataClient` trait with `KisMarketDataClient` (live HTTP) and `MockMarketDataClient` (test sequences). `MockMarketDataClient` replays a `VecDeque` of prices per symbol, repeating the last value when exhausted.

- **`src/order.rs`** — `OrderClient` trait with `PaperOrderClient` (no-op) and `LiveOrderClient` (KIS HTTP). KIS tr_ids: `TTTC0802U` (buy), `TTTC0801U` (sell).

- **`src/config.rs`** — Loads `config.toml` via TOML deserialization. `Credentials` reads `KIS_APP_KEY`, `KIS_APP_SECRET`, `KIS_ACCOUNT_NO` from environment (panics if missing).

- **`src/logging.rs`** — Dual output: stdout and daily-rotating file under `logs/protrader.log.YYYY-MM-DD`. No timestamps in log format (timestamps are embedded in log messages as KST strings).

### Key design decisions

- Prices are `i64` (Korean Won, integer). No floating point for money.
- Daily loss limit is checked against **realized P&L only** (not unrealized) to avoid halting on temporary dips.
- Stop-loss blacklists the symbol for the session — no re-entry after a stop-loss exit.
- `StrategyEngine` is intentionally I/O-free; all async work lives in `scheduler.rs`.

### Configuration (`config.toml`)

| Key | Description |
|-----|-------------|
| `trading.mode` | `"paper"` or `"live"` |
| `trading.fixed_amount_krw` | Per-trade budget in KRW |
| `trading.breakout_buffer_pct` | % above range high to trigger buy |
| `trading.range_minutes` | Duration of opening range capture (minutes after 09:00) |
| `trading.poll_interval_secs` | Price polling frequency |
| `trading.exit_time` | End-of-day forced close time (`HH:MM`) |
| `risk.stop_loss_pct` | Stop-loss trigger (% below entry) |
| `risk.daily_loss_limit_krw` | Max realized loss per session before halting new entries |
| `symbols.watchlist` | KOSPI ticker codes (e.g. `"005930"` = Samsung) |

### Credentials (`.env`)

Copy `.env.example` to `.env` and set `KIS_APP_KEY`, `KIS_APP_SECRET`, `KIS_ACCOUNT_NO`. The live smoke test in `market_data.rs` is gated with `#[ignore]` and requires real credentials.
