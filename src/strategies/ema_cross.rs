use std::collections::HashMap;
use crate::config::{TradingConfig, RiskConfig, SymbolConfig};
use super::{ExitReason, Position, SessionPhase, SessionPnl, Signal, Strategy};

// ── EmaCrossStrategy ──────────────────────────────────────────────────────────
//
// Trades the classic EMA crossover: enter long when the fast EMA rises above
// the slow EMA ("golden cross"), exit when it falls back below ("death cross")
// or when stop-loss / daily-limit triggers first.
//
// Both EMAs are seeded with the first tick price and updated on every tick
// (including the CapturingRange phase so they have enough history by the time
// Monitoring begins).

struct EmaCrossState {
    fast_alpha: f64,
    slow_alpha: f64,
    fast_ema: Option<f64>,
    slow_ema: Option<f64>,
    position: Option<Position>,
    blacklisted: bool,
    cached_unrealized: i64,
    fixed_amount: i64,
    stop_loss_pct: f64,
}

impl EmaCrossState {
    fn new(fixed_amount: i64, stop_loss_pct: f64, fast_period: u32, slow_period: u32) -> Self {
        Self {
            fast_alpha: 2.0 / (fast_period as f64 + 1.0),
            slow_alpha: 2.0 / (slow_period as f64 + 1.0),
            fast_ema: None,
            slow_ema: None,
            position: None,
            blacklisted: false,
            cached_unrealized: 0,
            fixed_amount,
            stop_loss_pct,
        }
    }

    fn update_emas(&mut self, price: i64) {
        let p = price as f64;
        self.fast_ema = Some(match self.fast_ema {
            Some(prev) => self.fast_alpha * p + (1.0 - self.fast_alpha) * prev,
            None => p,
        });
        self.slow_ema = Some(match self.slow_ema {
            Some(prev) => self.slow_alpha * p + (1.0 - self.slow_alpha) * prev,
            None => p,
        });
    }
}

pub struct EmaCrossStrategy {
    symbols: HashMap<String, EmaCrossState>,
    session_pnl: SessionPnl,
}

impl EmaCrossStrategy {
    pub fn new(
        trading: &TradingConfig,
        risk: &RiskConfig,
        symbols: &[SymbolConfig],
        fast_period: u32,
        slow_period: u32,
    ) -> Self {
        let states = symbols
            .iter()
            .map(|sc| {
                let state = EmaCrossState::new(
                    sc.effective_fixed_amount(trading),
                    sc.effective_stop_loss_pct(risk),
                    fast_period,
                    slow_period,
                );
                (sc.ticker.clone(), state)
            })
            .collect();
        Self { symbols: states, session_pnl: SessionPnl::default() }
    }
}

impl Strategy for EmaCrossStrategy {
    fn on_tick(&mut self, symbol: &str, price: i64, phase: &SessionPhase, daily_limit_hit: bool) -> Signal {
        if let Some(state) = self.symbols.get_mut(symbol) {
            state.cached_unrealized =
                state.position.as_ref().map(|p| p.unrealized_pnl(price)).unwrap_or(0);
        }
        self.session_pnl.unrealized = self.symbols.values().map(|s| s.cached_unrealized).sum();

        let state = match self.symbols.get_mut(symbol) {
            Some(s) => s,
            None => return Signal::Hold,
        };

        // Keep EMAs warm in all non-Closed phases.
        if !matches!(phase, SessionPhase::Closed) {
            state.update_emas(price);
        }

        match phase {
            SessionPhase::CapturingRange => Signal::Hold,

            SessionPhase::Monitoring => {
                // Priority 1: stop-loss
                if let Some(ref pos) = state.position {
                    let stop =
                        (pos.entry_price as f64 * (1.0 - state.stop_loss_pct / 100.0)) as i64;
                    if price <= stop {
                        return Signal::Exit { price, reason: ExitReason::StopLoss };
                    }
                }
                // Priority 2: daily limit
                if daily_limit_hit {
                    if state.position.is_some() {
                        return Signal::Exit { price, reason: ExitReason::DailyLimitReached };
                    }
                    return Signal::Hold;
                }

                let (fast, slow) = match (state.fast_ema, state.slow_ema) {
                    (Some(f), Some(s)) => (f, s),
                    _ => return Signal::Hold,
                };

                // Priority 3: death cross — exit open position
                if state.position.is_some() && fast <= slow {
                    return Signal::Exit { price, reason: ExitReason::SignalExit };
                }
                // Priority 4: golden cross — enter
                if state.position.is_none() && !state.blacklisted && fast > slow {
                    let qty = (state.fixed_amount / price) as u32;
                    if qty > 0 {
                        return Signal::Buy { price, qty };
                    }
                }
                Signal::Hold
            }

            SessionPhase::Closed => {
                if state.position.is_some() {
                    return Signal::Exit { price, reason: ExitReason::ForcedClose };
                }
                Signal::Hold
            }
        }
    }

    fn record_buy(&mut self, symbol: &str, price: i64, qty: u32) {
        if let Some(state) = self.symbols.get_mut(symbol) {
            if state.position.is_none() {
                state.position = Some(Position { entry_price: price, qty });
            }
        }
    }

    fn record_exit(&mut self, symbol: &str, price: i64, blacklist: bool) -> i64 {
        let state = match self.symbols.get_mut(symbol) {
            Some(s) => s,
            None => return 0,
        };
        let pnl = state.position.take().map(|p| p.realized_pnl(price)).unwrap_or(0);
        state.cached_unrealized = 0;
        if blacklist {
            state.blacklisted = true;
        }
        self.session_pnl.realized += pnl;
        self.session_pnl.unrealized = self.symbols.values().map(|s| s.cached_unrealized).sum();
        pnl
    }

    fn get_position_qty(&self, symbol: &str) -> u32 {
        self.symbols
            .get(symbol)
            .and_then(|s| s.position.as_ref())
            .map(|p| p.qty)
            .unwrap_or(0)
    }

    fn session_pnl(&self) -> SessionPnl {
        self.session_pnl.clone()
    }

    fn reset(&mut self) {
        for state in self.symbols.values_mut() {
            state.fast_ema = None;
            state.slow_ema = None;
            state.position = None;
            state.blacklisted = false;
            state.cached_unrealized = 0;
        }
        self.session_pnl = SessionPnl::default();
    }
}
