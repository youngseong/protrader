use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use chrono::{Timelike, TimeZone};
use chrono_tz::Asia::Seoul;

use crate::config::{Config, TradingMode};
use crate::strategy::{StrategyEngine, SessionPhase, Signal, ExitReason};
use crate::order::{OrderClient, OrderRequest, OrderSide};
use crate::market_data::MarketDataClient;

pub struct SessionScheduler {
    config: Arc<Config>,
    engine: Arc<Mutex<StrategyEngine>>,
    market_data: Arc<dyn MarketDataClient>,
    order_client: Arc<dyn OrderClient>,
}

impl SessionScheduler {
    pub fn new(
        config: Arc<Config>,
        engine: Arc<Mutex<StrategyEngine>>,
        market_data: Arc<dyn MarketDataClient>,
        order_client: Arc<dyn OrderClient>,
    ) -> Self {
        Self { config, engine, market_data, order_client }
    }

    pub async fn run(&self) -> anyhow::Result<()> {
        let mode_str = match self.config.trading.mode {
            TradingMode::Paper => "PAPER",
            TradingMode::Live => "LIVE",
        };

        // ── If market is already closed today, wait until tomorrow 09:00 ───
        let (exit_h, exit_m) = parse_time(&self.config.trading.exit_time);
        {
            let now = chrono::Utc::now().with_timezone(&Seoul);
            let now_min = now.hour() * 60 + now.minute();
            if now_min >= exit_h * 60 + exit_m {
                let tomorrow = now.date_naive().succ_opt().expect("date overflow");
                let open_tomorrow = Seoul
                    .from_local_datetime(&tomorrow.and_hms_opt(9, 0, 0).unwrap())
                    .unwrap();
                let wait_secs = (open_tomorrow - chrono::Utc::now().with_timezone(&Seoul))
                    .num_seconds()
                    .max(0) as u64;
                tracing::info!(
                    "Market closed for today — next session starts {} | waiting {}s",
                    open_tomorrow.format("%Y-%m-%d 09:00 KST"),
                    wait_secs
                );
                sleep(Duration::from_secs(wait_secs)).await;
            }
        }

        // ── Wait until 09:00 KST ────────────────────────────────────────────
        self.wait_until(9, 0).await;
        tracing::info!("Session started — capturing opening range until 09:{:02}", self.config.trading.range_minutes);

        let shutdown = Arc::new(AtomicBool::new(false));

        // ── Spawn one task per symbol ────────────────────────────────────────
        let handles: Vec<_> = self
            .config
            .symbols
            .watchlist
            .iter()
            .map(|symbol| {
                let symbol = symbol.clone();
                let engine = self.engine.clone();
                let market_data = self.market_data.clone();
                let order_client = self.order_client.clone();
                let poll = Duration::from_secs(self.config.trading.poll_interval_secs);
                let mode = mode_str.to_string();
                let shutdown = shutdown.clone();
                tokio::spawn(async move {
                    symbol_loop(symbol, engine, market_data, order_client, poll, mode, shutdown).await;
                })
            })
            .collect();

        // ── Wait until end of range window → switch to Monitoring ───────────
        self.wait_until(9, self.config.trading.range_minutes).await;
        self.engine.lock().await.set_phase(SessionPhase::Monitoring);
        tracing::info!("Opening range locked — monitoring for breakouts");

        // ── Wait until exit_time → switch to Closed ─────────────────────────
        self.wait_until(exit_h, exit_m).await;
        self.engine.lock().await.set_phase(SessionPhase::Closed);
        tracing::info!("Exit time reached — forcing close of all open positions");

        // Give symbol tasks one extra poll cycle to process the ForcedClose signals
        sleep(Duration::from_secs(self.config.trading.poll_interval_secs + 2)).await;

        // Signal all tasks to stop
        shutdown.store(true, Ordering::Relaxed);
        for h in handles {
            let _ = h.await;
        }

        let pnl = self.engine.lock().await.session_pnl.clone();
        tracing::info!(
            "Session complete | realized={} | unrealized={} | total={}",
            pnl.realized,
            pnl.unrealized,
            pnl.total()
        );

        Ok(())
    }

    /// Sleep until a given HH:MM KST wall-clock time. Returns immediately if already past.
    async fn wait_until(&self, hour: u32, minute: u32) {
        loop {
            let now = chrono::Utc::now().with_timezone(&Seoul);
            let now_min = now.hour() * 60 + now.minute();
            let target_min = hour * 60 + minute;
            if now_min >= target_min {
                return;
            }
            let secs_remaining = (target_min - now_min) * 60 - now.second();
            tracing::info!("Waiting {}s until {:02}:{:02} KST", secs_remaining, hour, minute);
            sleep(Duration::from_secs(secs_remaining as u64)).await;
        }
    }
}

async fn symbol_loop(
    symbol: String,
    engine: Arc<Mutex<StrategyEngine>>,
    market_data: Arc<dyn MarketDataClient>,
    order_client: Arc<dyn OrderClient>,
    poll: Duration,
    mode_str: String,
    shutdown: Arc<AtomicBool>,
) {
    let mut retry_count = 0u32;

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        sleep(poll).await;

        let price = match market_data.fetch_price(&symbol).await {
            Ok(p) => {
                retry_count = 0;
                p
            }
            Err(e) => {
                retry_count += 1;
                tracing::warn!(
                    "symbol={} price fetch error (attempt {}): {}",
                    symbol, retry_count, e
                );
                if retry_count >= 3 {
                    tracing::warn!("symbol={} skipping tick after 3 failures, backing off", symbol);
                    sleep(Duration::from_secs(2u64.pow(retry_count.min(6)))).await;
                    retry_count = 0;
                } else {
                    sleep(Duration::from_secs(2u64.pow(retry_count))).await;
                }
                continue;
            }
        };

        let signal = engine.lock().await.on_tick(&symbol, price);

        match signal {
            Signal::Buy { price, qty } => {
                let amount = price * qty as i64;
                let ts = now_kst();
                tracing::info!(
                    "{} KST | [{}] | BUY {} | price={} | qty={} | amount={}",
                    ts, mode_str, symbol, price, qty, amount
                );
                let req = OrderRequest { symbol: symbol.clone(), side: OrderSide::Buy, qty, price };
                match order_client.place_order(&req).await {
                    Ok(_) => {
                        engine.lock().await.record_buy(&symbol, price, qty);
                    }
                    Err(e) => {
                        tracing::error!(
                            "{} KST | [{}] | ORDER FAILED {} | error={}",
                            now_kst(), mode_str, symbol, e
                        );
                    }
                }
            }

            Signal::Exit { price, reason } => {
                let blacklist = matches!(reason, ExitReason::StopLoss);
                let qty = engine.lock().await.get_position_qty(&symbol);

                if qty == 0 {
                    tracing::warn!(
                        "{} KST | [{}] | EXIT {} ignored — no open position",
                        now_kst(), mode_str, symbol
                    );
                } else {
                    let req = OrderRequest { symbol: symbol.clone(), side: OrderSide::Sell, qty, price };
                    match order_client.place_order(&req).await {
                        Ok(_) => {
                            let pnl = engine.lock().await.record_exit(&symbol, price, blacklist);
                            let reason_str = match &reason {
                                ExitReason::StopLoss => "STOP-LOSS",
                                ExitReason::DailyLimitReached => "DAILY-LIMIT",
                                ExitReason::ForcedClose => "FORCED-CLOSE",
                            };
                            let blacklist_suffix = if blacklist { " | blacklisted" } else { "" };
                            tracing::info!(
                                "{} KST | [{}] | {} {} | price={} | realized_pnl={}{}",
                                now_kst(), mode_str, reason_str, symbol, price, pnl, blacklist_suffix
                            );
                            let session = engine.lock().await.session_pnl.clone();
                            tracing::info!(
                                "{} KST | [{}] | SESSION PnL | realized={} | unrealized={} | total={}",
                                now_kst(), mode_str, session.realized, session.unrealized, session.total()
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                "{} KST | [{}] | ORDER FAILED {} | error={}",
                                now_kst(), mode_str, symbol, e
                            );
                            // Position remains open — do NOT call record_exit
                        }
                    }
                }
            }

            Signal::Hold => {}
        }
    }
}

fn now_kst() -> String {
    chrono::Utc::now()
        .with_timezone(&Seoul)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

pub fn parse_time(s: &str) -> (u32, u32) {
    let parts: Vec<&str> = s.splitn(2, ':').collect();
    let h: u32 = parts[0].parse().expect("invalid exit_time hour");
    let m: u32 = parts[1].parse().expect("invalid exit_time minute");
    (h, m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_time_standard() {
        assert_eq!(parse_time("15:20"), (15, 20));
    }

    #[test]
    fn test_parse_time_zero_padded() {
        assert_eq!(parse_time("09:00"), (9, 0));
    }
}
