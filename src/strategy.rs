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

                // Priority 2: if daily limit hit, close any open position and block new entries
                if self.daily_limit_hit {
                    if state.position.is_some() {
                        return Signal::Exit {
                            price,
                            reason: ExitReason::DailyLimitReached,
                        };
                    }
                    return Signal::Hold;
                }

                // Priority 3: no new entries if position open or blacklisted
                if state.position.is_some() || state.blacklisted {
                    return Signal::Hold;
                }

                // Priority 4: check for breakout entry
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
            if state.position.is_some() {
                tracing::warn!("record_buy called for {} but position already open — ignoring", symbol);
                return;
            }
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
        // Daily limit is checked against realized P&L only (not unrealized), per spec.
        // Unrealized losses fluctuate and would cause premature halting on temporary dips.
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
    fn test_daily_limit_hit_closes_open_positions() {
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

        // Buy 069500 and keep it open
        engine.record_buy("069500", 9_200, 54); // 500_000/9_200=54

        // Take a huge realized loss on 005930 that triggers the daily limit
        engine.record_buy("005930", 72_000, 6);
        engine.record_exit("005930", 55_000, false); // realized = (55_000-72_000)*6 = -102_000

        assert!(engine.daily_limit_hit());

        // Next tick for 069500 should emit DailyLimitReached to close the open position
        let signal = engine.on_tick("069500", 9_300);
        assert_eq!(signal, Signal::Exit { price: 9_300, reason: ExitReason::DailyLimitReached });
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
