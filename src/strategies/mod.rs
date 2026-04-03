pub mod ema_cross;
pub mod orb;
pub mod vwap_reversion;

pub use ema_cross::EmaCrossStrategy;
pub use orb::OrbStrategy;
pub use vwap_reversion::VwapReversionStrategy;

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum ExitReason {
    StopLoss,
    DailyLimitReached,
    ForcedClose,
    /// Strategy-internal signal triggered exit (e.g. EMA death cross, VWAP reversion).
    SignalExit,
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

// ── Strategy trait ────────────────────────────────────────────────────────────

pub trait Strategy: Send {
    fn on_tick(&mut self, symbol: &str, price: i64, phase: &SessionPhase, daily_limit_hit: bool) -> Signal;
    fn record_buy(&mut self, symbol: &str, price: i64, qty: u32);
    /// Records a sell and returns the realized PnL for this trade.
    fn record_exit(&mut self, symbol: &str, price: i64, blacklist: bool) -> i64;
    fn get_position_qty(&self, symbol: &str) -> u32;
    fn session_pnl(&self) -> SessionPnl;
    /// Reset all per-session state so the strategy is ready for a new trading day.
    fn reset(&mut self);
}

// ── StrategyEngine (coordinator) ──────────────────────────────────────────────

pub struct StrategyEngine {
    strategy: Box<dyn Strategy>,
    phase: SessionPhase,
    daily_limit_hit: bool,
    daily_loss_limit: i64,
}

impl StrategyEngine {
    pub fn new(strategy: Box<dyn Strategy>, daily_loss_limit: i64) -> Self {
        Self {
            strategy,
            phase: SessionPhase::CapturingRange,
            daily_limit_hit: false,
            daily_loss_limit,
        }
    }

    pub fn set_phase(&mut self, phase: SessionPhase) {
        self.phase = phase;
    }

    /// Reset all per-session state for a new trading day.
    pub fn reset(&mut self) {
        self.strategy.reset();
        self.phase = SessionPhase::CapturingRange;
        self.daily_limit_hit = false;
    }

    pub fn on_tick(&mut self, symbol: &str, price: i64) -> Signal {
        self.strategy.on_tick(symbol, price, &self.phase, self.daily_limit_hit)
    }

    pub fn record_buy(&mut self, symbol: &str, price: i64, qty: u32) {
        self.strategy.record_buy(symbol, price, qty);
    }

    pub fn record_exit(&mut self, symbol: &str, price: i64, blacklist: bool) -> i64 {
        let pnl = self.strategy.record_exit(symbol, price, blacklist);
        // Daily limit is checked against realized P&L only (not unrealized).
        if self.strategy.session_pnl().realized < -self.daily_loss_limit {
            self.daily_limit_hit = true;
        }
        pnl
    }

    pub fn get_position_qty(&self, symbol: &str) -> u32 {
        self.strategy.get_position_qty(symbol)
    }

    pub fn daily_limit_hit(&self) -> bool {
        self.daily_limit_hit
    }

    pub fn session_pnl(&self) -> SessionPnl {
        self.strategy.session_pnl()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{TradingConfig, RiskConfig, TradingMode, SymbolConfig};

    fn make_trading() -> TradingConfig {
        TradingConfig {
            mode: TradingMode::Paper,
            fixed_amount: 500_000,
            breakout_buffer_pct: 0.2,
            range_minutes: 30,
            poll_interval_secs: 5,
        }
    }

    fn make_risk() -> RiskConfig {
        RiskConfig { stop_loss_pct: 1.5, daily_loss_limit: 100_000 }
    }

    fn sym(ticker: &str) -> SymbolConfig {
        SymbolConfig { ticker: ticker.to_string(), fixed_amount: None, breakout_buffer_pct: None, stop_loss_pct: None }
    }

    fn make_engine() -> StrategyEngine {
        let trading = make_trading();
        let risk = make_risk();
        let orb = OrbStrategy::new(&trading, &risk, &[sym("005930")]);
        StrategyEngine::new(Box::new(orb), risk.daily_loss_limit)
    }

    #[test]
    fn test_no_buy_during_range_capture() {
        let mut engine = make_engine();
        let signal = engine.on_tick("005930", 99_000);
        assert_eq!(signal, Signal::Hold);
    }

    #[test]
    fn test_buy_signal_on_breakout() {
        let mut engine = make_engine();
        engine.on_tick("005930", 71_000); // sets range_high = 71_000
        engine.set_phase(SessionPhase::Monitoring);
        // breakout_price = 71_000 * 1.002 = 71_142; price 72_000 > 71_142 → Buy; qty = 500_000 / 72_000 = 6
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
        let signal = engine.on_tick("005930", 75_000);
        assert_eq!(signal, Signal::Hold);
    }

    #[test]
    fn test_stop_loss_triggers_exit() {
        let mut engine = make_engine();
        engine.on_tick("005930", 71_000);
        engine.set_phase(SessionPhase::Monitoring);
        engine.record_buy("005930", 72_000, 6);
        // stop_loss at 72_000 * (1 - 0.015) = 70_920; 70_800 <= 70_920
        let signal = engine.on_tick("005930", 70_800);
        assert_eq!(signal, Signal::Exit { price: 70_800, reason: ExitReason::StopLoss });
    }

    #[test]
    fn test_record_exit_calculates_realized_pnl() {
        let mut engine = make_engine();
        engine.on_tick("005930", 71_000);
        engine.set_phase(SessionPhase::Monitoring);
        engine.record_buy("005930", 72_000, 6);
        let pnl = engine.record_exit("005930", 73_000, false);
        assert_eq!(pnl, (73_000 - 72_000) * 6); // +6_000
        assert_eq!(engine.session_pnl().realized, 6_000);
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
    }

    #[test]
    fn test_daily_loss_limit_stops_new_entries() {
        let trading = make_trading();
        let risk = make_risk();
        let orb = OrbStrategy::new(&trading, &risk, &[sym("005930"), sym("069500")]);
        let mut engine = StrategyEngine::new(Box::new(orb), risk.daily_loss_limit);

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
        assert_eq!(signal, Signal::Exit { price: 72_500, reason: ExitReason::ForcedClose });
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
        let trading = make_trading();
        let risk = make_risk();
        let orb = OrbStrategy::new(&trading, &risk, &[sym("005930"), sym("069500")]);
        let mut engine = StrategyEngine::new(Box::new(orb), risk.daily_loss_limit);

        engine.on_tick("005930", 71_000);
        engine.on_tick("069500", 9_000);
        engine.set_phase(SessionPhase::Monitoring);

        engine.record_buy("069500", 9_200, 54);
        engine.record_buy("005930", 72_000, 6);
        engine.record_exit("005930", 55_000, false); // realized = (55_000-72_000)*6 = -102_000

        assert!(engine.daily_limit_hit());

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
        assert_eq!(engine.session_pnl().unrealized, 6_000);
    }

    #[test]
    fn test_per_symbol_fixed_amount_override() {
        let trading = make_trading(); // fixed_amount = 500_000
        let risk = make_risk();
        let symbols = vec![
            sym("005930"),
            SymbolConfig {
                ticker: "069500".to_string(),
                fixed_amount: Some(200_000),
                breakout_buffer_pct: None,
                stop_loss_pct: None,
            },
        ];
        let orb = OrbStrategy::new(&trading, &risk, &symbols);
        let mut engine = StrategyEngine::new(Box::new(orb), risk.daily_loss_limit);

        engine.on_tick("069500", 9_000);
        engine.set_phase(SessionPhase::Monitoring);
        // breakout_price = 9_000 * 1.002 = 9_018; price 10_000 > 9_018
        // qty = 200_000 / 10_000 = 20 (not 500_000 / 10_000 = 50)
        let signal = engine.on_tick("069500", 10_000);
        assert_eq!(signal, Signal::Buy { price: 10_000, qty: 20 });
    }
}
