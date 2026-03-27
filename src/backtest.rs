use std::collections::HashMap;
use std::sync::Arc;

use crate::config::Config;
use crate::historical::Tick;
use crate::order::{OrderClient, OrderRequest, OrderSide, PaperOrderClient};
use crate::strategy::{ExitReason, SessionPhase, Signal, Strategy, StrategyEngine};

// ── Public result type ────────────────────────────────────────────────────────

pub struct BacktestResult {
    pub name: String,
    /// Realized P&L from closed trades (KRW).
    pub realized_pnl: i64,
    /// Unrealized P&L on any positions still open at end of session (should be 0).
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
        });
    }

    /// Drive all registered runs through `ticks` (must be sorted ascending by time).
    ///
    /// Phase transitions follow `config`:
    ///   open_time                → CapturingRange
    ///   open_time + range_minutes → Monitoring
    ///   exit_time                → Closed
    ///
    /// Any positions still open after the final tick are force-closed at the
    /// last known price for that symbol.
    pub async fn run(&mut self, ticks: &[Tick]) -> Vec<BacktestResult> {
        let open = self.config.market.open_time;
        let range_end =
            open + chrono::Duration::minutes(self.config.trading.range_minutes as i64);
        let exit = self.config.market.exit_time;

        // Track the last seen price per symbol for the final force-close pass.
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

            for run in &mut self.runs {
                run.engine.set_phase(phase.clone());
                let signal = run.engine.on_tick(&tick.symbol, tick.price);
                let traded =
                    dispatch(signal, &tick.symbol, &mut run.engine, &run.order_client).await;
                run.trade_count += traded;
            }
        }

        // Force-close any positions that survived past the last tick.
        for (symbol, price) in &last_prices {
            for run in &mut self.runs {
                run.engine.set_phase(SessionPhase::Closed);
                let signal = run.engine.on_tick(symbol, *price);
                let traded =
                    dispatch(signal, symbol, &mut run.engine, &run.order_client).await;
                run.trade_count += traded;
            }
        }

        self.runs
            .iter()
            .map(|run| {
                let pnl = run.engine.session_pnl();
                BacktestResult {
                    name: run.name.clone(),
                    realized_pnl: pnl.realized,
                    unrealized_pnl: pnl.unrealized,
                    trade_count: run.trade_count,
                }
            })
            .collect()
    }
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
