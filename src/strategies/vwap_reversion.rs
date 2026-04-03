use std::collections::HashMap;
use crate::config::{TradingConfig, RiskConfig, SymbolConfig};
use super::{ExitReason, Position, SessionPhase, SessionPnl, Signal, Strategy};

// ── VwapReversionStrategy ─────────────────────────────────────────────────────
//
// Computes a running session VWAP proxy (equal-weighted price average, since
// per-tick volume is unavailable from minute-bar data). Enters long when price
// drops more than `entry_deviation_pct` below the running average; exits when
// price reverts back to or above the average, or when stop-loss / daily-limit
// fires first.
//
// The average starts accumulating from the first tick after market open
// (CapturingRange phase), giving it meaningful context before Monitoring begins.

struct VwapReversionState {
    price_sum: f64,
    tick_count: u64,
    position: Option<Position>,
    blacklisted: bool,
    cached_unrealized: i64,
    fixed_amount: i64,
    stop_loss_pct: f64,
    entry_deviation_pct: f64,
}

impl VwapReversionState {
    fn new(fixed_amount: i64, stop_loss_pct: f64, entry_deviation_pct: f64) -> Self {
        Self {
            price_sum: 0.0,
            tick_count: 0,
            position: None,
            blacklisted: false,
            cached_unrealized: 0,
            fixed_amount,
            stop_loss_pct,
            entry_deviation_pct,
        }
    }

    fn vwap(&self) -> Option<f64> {
        if self.tick_count > 0 { Some(self.price_sum / self.tick_count as f64) } else { None }
    }
}

pub struct VwapReversionStrategy {
    symbols: HashMap<String, VwapReversionState>,
    session_pnl: SessionPnl,
}

impl VwapReversionStrategy {
    pub fn new(
        trading: &TradingConfig,
        risk: &RiskConfig,
        symbols: &[SymbolConfig],
        entry_deviation_pct: f64,
    ) -> Self {
        let states = symbols
            .iter()
            .map(|sc| {
                let state = VwapReversionState::new(
                    sc.effective_fixed_amount(trading),
                    sc.effective_stop_loss_pct(risk),
                    entry_deviation_pct,
                );
                (sc.ticker.clone(), state)
            })
            .collect();
        Self { symbols: states, session_pnl: SessionPnl::default() }
    }
}

impl Strategy for VwapReversionStrategy {
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

        // Accumulate every tick (range capture + monitoring) for the VWAP average.
        if !matches!(phase, SessionPhase::Closed) {
            state.price_sum += price as f64;
            state.tick_count += 1;
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

                let vwap = match state.vwap() {
                    Some(v) => v,
                    None => return Signal::Hold,
                };

                // Priority 3: exit when price reverts to VWAP
                if state.position.is_some() && price as f64 >= vwap {
                    return Signal::Exit { price, reason: ExitReason::SignalExit };
                }
                // Priority 4: buy when price drops below VWAP by threshold
                if state.position.is_none() && !state.blacklisted {
                    let buy_threshold = vwap * (1.0 - state.entry_deviation_pct / 100.0);
                    if (price as f64) < buy_threshold {
                        let qty = (state.fixed_amount / price) as u32;
                        if qty > 0 {
                            return Signal::Buy { price, qty };
                        }
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
            state.price_sum = 0.0;
            state.tick_count = 0;
            state.position = None;
            state.blacklisted = false;
            state.cached_unrealized = 0;
        }
        self.session_pnl = SessionPnl::default();
    }
}
