use std::collections::HashMap;
use crate::config::{TradingConfig, RiskConfig, SymbolConfig};
use super::{ExitReason, Position, SessionPhase, SessionPnl, Signal, Strategy};

struct SymbolState {
    // resolved per-symbol config (overrides or globals)
    fixed_amount: i64,
    breakout_buffer_pct: f64,
    stop_loss_pct: f64,
    // runtime state
    range_high: i64,
    range_low: i64,
    position: Option<Position>,
    blacklisted: bool,
    cached_unrealized: i64,
}

impl SymbolState {
    fn new(fixed_amount: i64, breakout_buffer_pct: f64, stop_loss_pct: f64) -> Self {
        Self {
            fixed_amount,
            breakout_buffer_pct,
            stop_loss_pct,
            range_high: 0,
            range_low: i64::MAX,
            position: None,
            blacklisted: false,
            cached_unrealized: 0,
        }
    }
}

// ── OrbStrategy ───────────────────────────────────────────────────────────────

pub struct OrbStrategy {
    symbols: HashMap<String, SymbolState>,
    session_pnl: SessionPnl,
}

impl OrbStrategy {
    pub fn new(trading: &TradingConfig, risk: &RiskConfig, symbols: &[SymbolConfig]) -> Self {
        let states = symbols
            .iter()
            .map(|sc| {
                let state = SymbolState::new(
                    sc.effective_fixed_amount(trading),
                    sc.effective_breakout_buffer_pct(trading),
                    sc.effective_stop_loss_pct(risk),
                );
                (sc.ticker.clone(), state)
            })
            .collect();
        Self { symbols: states, session_pnl: SessionPnl::default() }
    }
}

impl Strategy for OrbStrategy {
    fn on_tick(&mut self, symbol: &str, price: i64, phase: &SessionPhase, daily_limit_hit: bool) -> Signal {
        // Step 1: update this symbol's cached unrealized (ends borrow)
        if let Some(state) = self.symbols.get_mut(symbol) {
            state.cached_unrealized = state
                .position
                .as_ref()
                .map(|p| p.unrealized_pnl(price))
                .unwrap_or(0);
        }
        // Step 2: recompute total session unrealized (safe: no active borrow)
        self.session_pnl.unrealized = self.symbols.values().map(|s| s.cached_unrealized).sum();

        let state = match self.symbols.get_mut(symbol) {
            Some(s) => s,
            None => return Signal::Hold,
        };

        match phase {
            SessionPhase::CapturingRange => {
                if price > state.range_high { state.range_high = price; }
                if price < state.range_low { state.range_low = price; }
                Signal::Hold
            }

            SessionPhase::Monitoring => {
                // Priority 1: stop-loss on open position (always checked)
                if let Some(ref pos) = state.position {
                    let stop_price =
                        (pos.entry_price as f64 * (1.0 - state.stop_loss_pct / 100.0)) as i64;
                    if price <= stop_price {
                        return Signal::Exit { price, reason: ExitReason::StopLoss };
                    }
                }
                // Priority 2: daily limit — close open position, block new entries
                if daily_limit_hit {
                    if state.position.is_some() {
                        return Signal::Exit { price, reason: ExitReason::DailyLimitReached };
                    }
                    return Signal::Hold;
                }
                // Priority 3: no re-entry while position open or symbol blacklisted
                if state.position.is_some() || state.blacklisted {
                    return Signal::Hold;
                }
                // Priority 4: check for breakout entry
                if state.range_high > 0 {
                    let breakout_price = (state.range_high as f64
                        * (1.0 + state.breakout_buffer_pct / 100.0)) as i64;
                    if price > breakout_price {
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
            if state.position.is_some() {
                tracing::warn!("record_buy called for {} but position already open — ignoring", symbol);
                return;
            }
            state.position = Some(Position { entry_price: price, qty });
        }
    }

    fn record_exit(&mut self, symbol: &str, price: i64, blacklist: bool) -> i64 {
        let state = match self.symbols.get_mut(symbol) {
            Some(s) => s,
            None => return 0,
        };
        let pnl = state.position.take().map(|p| p.realized_pnl(price)).unwrap_or(0);
        state.cached_unrealized = 0;
        if blacklist { state.blacklisted = true; }
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
            state.range_high = 0;
            state.range_low = i64::MAX;
            state.position = None;
            state.blacklisted = false;
            state.cached_unrealized = 0;
        }
        self.session_pnl = SessionPnl::default();
    }
}
