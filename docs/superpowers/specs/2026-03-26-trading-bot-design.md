# ProTrader — KIS Opening Range Breakout Bot Design

**Date:** 2026-03-26
**Language:** Rust
**Exchange:** KRX (Korean stocks + ETFs)
**Strategy:** Opening Range Breakout (ORB)

---

## Overview

A Rust-based automated trading bot that trades Korean stocks and ETFs on KRX using the Opening Range Breakout strategy. The bot captures the price range during the first 30 minutes of the session (9:00–9:30 KST), then trades breakouts for the rest of the day. It starts in paper trading mode and can be switched to live trading via config.

---

## Architecture

Four clearly separated layers:

```
┌─────────────────────────────────────────┐
│           Scheduler / Main Loop         │  (drives session lifecycle)
├─────────────────────────────────────────┤
│           Strategy Engine               │  (ORB logic per symbol)
├─────────────────────────────────────────┤
│           Market Data Client            │  (KIS OpenAPI HTTP calls)
├─────────────────────────────────────────┤
│           Order Client                  │  (paper or live, same interface)
└─────────────────────────────────────────┘
```

- **Scheduler:** Knows the KST session clock. Waits until 9:00, captures range until 9:30, monitors until 15:30, then shuts down for the day.
- **Strategy Engine:** Holds per-symbol state (`RangeHigh`, `RangeLow`, current position). Takes price ticks and emits `Signal::Buy`, `Signal::Sell`, `Signal::Exit`, or `Signal::Hold`.
- **Market Data Client:** Polls the KIS price API on a configurable interval (default 5s). Each symbol runs as its own `tokio::task`.
- **Order Client:** A trait with two implementations — `PaperOrderClient` (logs to stdout/file) and `LiveOrderClient` (calls KIS order API). Switched via `config.toml`.

Runtime: `tokio` async, one task per symbol.

---

## Data Flow

```
9:00 KST — Scheduler spawns one tokio::task per symbol
     │
     ▼
9:00–9:30 — Market Data Client polls price every 5s
     │       Strategy Engine accumulates high/low → builds OpeningRange
     │
     ▼
9:30 — Range locked in (RangeHigh, RangeLow stored per symbol)
     │
     ▼
9:30–15:30 — Price tick arrives
     │        Strategy Engine checks:
     │          price > RangeHigh + buffer → Signal::Buy (long entry, if flat)
     │          stop-loss hit              → Signal::Exit + blacklist symbol
     │          daily loss limit hit       → Signal::Exit all positions
     │          15:20 KST                  → Signal::Exit (forced close)
     │
     ▼
Signal → Order Client
     │     PaperMode: log "WOULD BUY/SELL ..."
     │     LiveMode:  POST to KIS order endpoint
     │
     ▼
Position tracked in-memory (symbol, entry price, qty, unrealized P&L)
```

A `breakout_buffer_pct` (e.g., 0.2%) above range high / below range low prevents false breakouts on noise.

---

## Configuration

**`config.toml`** (non-sensitive settings only, git-tracked):

```toml
[trading]
mode = "paper"             # "paper" or "live"
fixed_amount_krw = 500000  # ₩500,000 per trade
breakout_buffer_pct = 0.2  # % above/below range to confirm breakout
range_minutes = 30         # opening range window (9:00–9:30)
poll_interval_secs = 5     # price polling frequency
exit_time = "15:20"        # forced position close time (KST)

[risk]
stop_loss_pct = 1.5            # exit position if price drops 1.5% below entry
daily_loss_limit_krw = 100000  # stop all trading if session P&L hits -₩100,000

[symbols]
watchlist = ["005930", "069500"]  # e.g. Samsung + KODEX 200 ETF
```

**Credentials via environment variables** (never in config.toml):

```
KIS_APP_KEY=...
KIS_APP_SECRET=...
```

A `.env` file (git-ignored) can hold these locally, loaded via the `dotenvy` crate. The app fails fast with a clear error if credentials are missing at startup.

---

## Risk Management

| Guard | Scope | Action |
|---|---|---|
| Stop-loss % | Per position | Exit immediately, blacklist symbol for the day |
| Daily loss limit | Whole session | Stop all new entries, close all open positions |

- **Stop-loss:** When a position's price drops `stop_loss_pct` below entry, the bot sells immediately and blacklists the symbol (no re-entry for the rest of the day).
- **Daily loss limit:** When total realized P&L for the session drops below `-daily_loss_limit_krw`, all open positions are closed and no new entries are taken.
- **Order failure (live mode):** Log the error, mark position as "pending confirmation", do NOT retry automatically.

---

## P&L Tracking

The bot tracks earnings throughout the session:

- **Unrealized P&L** per open position (current price vs entry price × qty)
- **Realized P&L** per closed trade
- **Session total P&L** (sum of all realized + unrealized)

P&L is logged on every signal and order event. State is in-memory only — no persistence across restarts. The bot is designed to start fresh each trading day.

---

## Logging

Uses the `tracing` crate with structured logs. Output goes to both stdout and a daily rotating file (`logs/YYYY-MM-DD.log`).

Log format:
```
2026-03-26 09:47:13 KST | [PAPER] | BUY 005930 | price=71400 | qty=7 | amount=499800
2026-03-26 10:15:42 KST | [PAPER] | SELL 005930 | price=72100 | qty=7 | realized_pnl=+4900
2026-03-26 10:31:05 KST | [PAPER] | STOP-LOSS 069500 | price=9850 | realized_pnl=-3200 | blacklisted
2026-03-26 10:31:05 KST | [PAPER] | SESSION PnL | realized=-3200 | unrealized=+4900 | total=+1700
```

---

## Error Handling

- **API failures (network, rate limit):** Retry up to 3 times with exponential backoff, then log and skip the tick. Never crash on transient errors.
- **Missing credentials at startup:** Panic immediately with a clear message (`KIS_APP_KEY not set`).
- **Invalid config values:** Fail fast at startup before market open.
- **Order failure in live mode:** Log error, mark position as "pending confirmation", no auto-retry.

---

## Testing

- **Unit tests — Strategy Engine:** Given a sequence of price ticks, assert correct signals (buy, sell, hold, stop-loss trigger, blacklist behavior, daily limit trigger).
- **Unit tests — P&L calculation:** Entry/exit prices → correct realized P&L, daily limit trigger.
- **Integration test:** `MockMarketDataClient` + `MockOrderClient` running a full simulated session against a canned price series; assert trade log matches expectations.
- **No live API tests:** KIS API tested manually in paper mode with real credentials.

---

## Key Dependencies

| Crate | Purpose |
|---|---|
| `tokio` | Async runtime |
| `reqwest` | HTTP client for KIS API |
| `serde` / `toml` | Config deserialization |
| `dotenvy` | Load `.env` for local credentials |
| `tracing` / `tracing-subscriber` | Structured logging |
| `chrono` | KST time handling |
| `anyhow` | Error handling |
