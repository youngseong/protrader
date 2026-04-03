use std::collections::HashMap;
use std::sync::Arc;

use chrono::NaiveDate;

use crate::config::Config;
use crate::historical::Tick;
use crate::order::{OrderClient, OrderRequest, OrderSide, PaperOrderClient};
use crate::strategy::{ExitReason, SessionPhase, Signal, Strategy, StrategyEngine};

// ── Public result type ────────────────────────────────────────────────────────

pub struct BacktestResult {
    pub name: String,
    /// Realized P&L from closed trades across all days (KRW).
    pub realized_pnl: i64,
    /// Unrealized P&L on positions still open at session end (should be 0).
    pub unrealized_pnl: i64,
    /// Number of round-trip trades completed (each exit = 1 trade).
    pub trade_count: u32,
}

// ── Internal per-run state ────────────────────────────────────────────────────

struct BacktestRun {
    name: String,
    engine: StrategyEngine,
    order_client: PaperOrderClient,
    trade_count: u32,
    /// Accumulated realized P&L from completed days (multi-day runs only).
    cumulative_realized_pnl: i64,
}

// ── Runner ────────────────────────────────────────────────────────────────────

pub struct BacktestRunner {
    runs: Vec<BacktestRun>,
    config: Arc<Config>,
}

impl BacktestRunner {
    pub fn new(config: Arc<Config>) -> Self {
        Self { runs: Vec::new(), config }
    }

    pub fn run_count(&self) -> usize {
        self.runs.len()
    }

    /// Register a strategy variant. Each variant gets its own independent
    /// `StrategyEngine` and `PaperOrderClient` so results are fully isolated.
    pub fn add_run(
        &mut self,
        name: impl Into<String>,
        strategy: Box<dyn Strategy>,
        initial_balance: i64,
    ) {
        self.runs.push(BacktestRun {
            name: name.into(),
            engine: StrategyEngine::new(strategy, self.config.risk.daily_loss_limit),
            order_client: PaperOrderClient::new(initial_balance),
            trade_count: 0,
            cumulative_realized_pnl: 0,
        });
    }

    /// Drive all registered runs through a single day's `ticks`
    /// (sorted ascending by time). Returns cumulative results.
    pub async fn run(&mut self, ticks: &[Tick]) -> Vec<BacktestResult> {
        run_session(&mut self.runs, ticks, &self.config).await;
        collect_results(&self.runs)
    }

    /// Drive all registered runs through a multi-day tick stream.
    ///
    /// Ticks are grouped by calendar date. Each day runs as an independent
    /// session: engines are reset between days so range, positions, blacklists,
    /// and the daily loss limit all start fresh. P&L is accumulated across days.
    pub async fn run_days(&mut self, ticks: &[Tick]) -> Vec<BacktestResult> {
        // Group ticks by date, preserving insertion (time) order within each day.
        let mut by_date: HashMap<NaiveDate, Vec<&Tick>> = HashMap::new();
        for tick in ticks {
            by_date.entry(tick.time.date()).or_default().push(tick);
        }

        let mut dates: Vec<NaiveDate> = by_date.keys().cloned().collect();
        dates.sort();

        for date in &dates {
            let day_ticks: Vec<Tick> =
                by_date[date].iter().map(|t| (*t).clone()).collect();

            run_session(&mut self.runs, &day_ticks, &self.config).await;

            // Fold this day's realized P&L into the cumulative total, then reset
            // so the next day starts fresh.
            for run in &mut self.runs {
                run.cumulative_realized_pnl += run.engine.session_pnl().realized;
                run.engine.reset();
            }
        }

        // `cumulative_realized_pnl` now holds the full multi-day total.
        self.runs
            .iter()
            .map(|run| BacktestResult {
                name: run.name.clone(),
                realized_pnl: run.cumulative_realized_pnl,
                unrealized_pnl: 0,
                trade_count: run.trade_count,
            })
            .collect()
    }
}

// ── Session loop ──────────────────────────────────────────────────────────────

/// Run one trading session worth of ticks through all runs.
/// Phase transitions are derived from config. Any open positions at the
/// end are force-closed at the last known price for that symbol.
async fn run_session(runs: &mut Vec<BacktestRun>, ticks: &[Tick], config: &Config) {
    let open = config.market.open_time;
    let range_end = open + chrono::Duration::minutes(config.trading.range_minutes as i64);
    let exit = config.market.exit_time;

    let mut last_prices: HashMap<String, i64> = HashMap::new();

    for tick in ticks {
        let t = tick.time.time();
        let phase = if t < open {
            continue;
        } else if t < range_end {
            SessionPhase::CapturingRange
        } else if t < exit {
            SessionPhase::Monitoring
        } else {
            SessionPhase::Closed
        };

        last_prices.insert(tick.symbol.clone(), tick.price);

        for run in runs.iter_mut() {
            run.engine.set_phase(phase.clone());
            let signal = run.engine.on_tick(&tick.symbol, tick.price);
            let traded =
                dispatch(signal, &tick.symbol, &mut run.engine, &run.order_client).await;
            run.trade_count += traded;
        }
    }

    // Force-close any positions still open after the last tick.
    for (symbol, price) in &last_prices {
        for run in runs.iter_mut() {
            run.engine.set_phase(SessionPhase::Closed);
            let signal = run.engine.on_tick(symbol, *price);
            let traded =
                dispatch(signal, symbol, &mut run.engine, &run.order_client).await;
            run.trade_count += traded;
        }
    }
}

fn collect_results(runs: &[BacktestRun]) -> Vec<BacktestResult> {
    runs.iter()
        .map(|run| {
            let pnl = run.engine.session_pnl();
            BacktestResult {
                name: run.name.clone(),
                realized_pnl: run.cumulative_realized_pnl + pnl.realized,
                unrealized_pnl: pnl.unrealized,
                trade_count: run.trade_count,
            }
        })
        .collect()
}

// ── Signal dispatch ───────────────────────────────────────────────────────────

/// Execute a signal against the engine and order client.
/// Returns 1 if a round-trip exit was completed, 0 otherwise.
async fn dispatch(
    signal: Signal,
    symbol: &str,
    engine: &mut StrategyEngine,
    order_client: &PaperOrderClient,
) -> u32 {
    match signal {
        Signal::Buy { price, qty } => {
            let req =
                OrderRequest { symbol: symbol.to_string(), side: OrderSide::Buy, qty, price };
            if order_client.place_order(&req).await.is_ok() {
                engine.record_buy(symbol, price, qty);
            }
            0
        }
        Signal::Exit { price, reason } => {
            let blacklist = matches!(reason, ExitReason::StopLoss);
            let qty = engine.get_position_qty(symbol);
            if qty > 0 {
                let req = OrderRequest {
                    symbol: symbol.to_string(),
                    side: OrderSide::Sell,
                    qty,
                    price,
                };
                if order_client.place_order(&req).await.is_ok() {
                    engine.record_exit(symbol, price, blacklist);
                    return 1;
                }
            }
            0
        }
        Signal::Hold => 0,
    }
}
