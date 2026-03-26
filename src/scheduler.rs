use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use chrono::{NaiveTime, TimeZone};

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
        let tz = self.config.market.timezone;
        let open_time = self.config.market.open_time;
        let exit_time = self.config.market.exit_time;

        // ── If market is already closed today, wait until tomorrow open ───────
        {
            let now = chrono::Utc::now().with_timezone(&tz);
            if now.time() >= exit_time {
                let tomorrow = now.date_naive().succ_opt().expect("date overflow");
                let open_naive = tomorrow.and_time(open_time);
                let open_local = tz.from_local_datetime(&open_naive).unwrap();
                let wait_secs = (open_local - chrono::Utc::now().with_timezone(&tz))
                    .num_seconds()
                    .max(0) as u64;
                tracing::info!(
                    "Market closed for today — next session starts {} | waiting {}s",
                    open_local.format("%Y-%m-%d %H:%M"),
                    wait_secs
                );
                sleep(Duration::from_secs(wait_secs)).await;
            }
        }

        // ── Wait until open_time ─────────────────────────────────────────────
        self.wait_until(open_time).await;
        let range_end = open_time
            + chrono::Duration::minutes(self.config.trading.range_minutes as i64);
        tracing::info!(
            "Session started — capturing opening range until {}",
            range_end.format("%H:%M")
        );

        let shutdown = Arc::new(AtomicBool::new(false));

        // ── Spawn one task per symbol ─────────────────────────────────────────
        let handles: Vec<_> = self
            .config
            .symbols
            .iter()
            .map(|sc| {
                let symbol = sc.ticker.clone();
                let engine = self.engine.clone();
                let market_data = self.market_data.clone();
                let order_client = self.order_client.clone();
                let poll = Duration::from_secs(self.config.trading.poll_interval_secs);
                let mode = mode_str.to_string();
                let shutdown = shutdown.clone();
                tokio::spawn(async move {
                    symbol_loop(symbol, engine, market_data, order_client, poll, mode, tz, shutdown).await;
                })
            })
            .collect();

        // ── Wait until end of range window → switch to Monitoring ─────────────
        self.wait_until(range_end).await;
        self.engine.lock().await.set_phase(SessionPhase::Monitoring);
        tracing::info!("Opening range locked — monitoring for breakouts");

        // ── Wait until exit_time → switch to Closed ──────────────────────────
        self.wait_until(exit_time).await;
        self.engine.lock().await.set_phase(SessionPhase::Closed);
        tracing::info!("Exit time reached — forcing close of all open positions");

        // Give symbol tasks one extra poll cycle to process the ForcedClose signals
        sleep(Duration::from_secs(self.config.trading.poll_interval_secs + 2)).await;

        shutdown.store(true, Ordering::Relaxed);
        for h in handles {
            let _ = h.await;
        }

        let pnl = self.engine.lock().await.session_pnl();
        tracing::info!(
            "Session complete | realized={} | unrealized={} | total={}",
            pnl.realized,
            pnl.unrealized,
            pnl.total()
        );

        Ok(())
    }

    /// Sleep until a given wall-clock time in the configured timezone.
    /// Returns immediately if already past.
    async fn wait_until(&self, target: NaiveTime) {
        let tz = self.config.market.timezone;
        loop {
            let now = chrono::Utc::now().with_timezone(&tz);
            let now_time = now.time();
            if now_time >= target {
                return;
            }
            let secs = (target - now_time).num_seconds().max(1) as u64;
            tracing::info!("Waiting {}s until {} {}", secs, target.format("%H:%M"), tz);
            sleep(Duration::from_secs(secs)).await;
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
    tz: chrono_tz::Tz,
    shutdown: Arc<AtomicBool>,
) {
    let mut retry_count = 0u32;

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        sleep(poll).await;

        let price = match market_data.fetch_price(&symbol).await {
            Ok(q) => {
                retry_count = 0;
                q.price
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
                let ts = now_local(tz);
                tracing::info!(
                    "{} | [{}] | BUY {} | price={} | qty={} | amount={}",
                    ts, mode_str, symbol, price, qty, amount
                );
                let req = OrderRequest { symbol: symbol.clone(), side: OrderSide::Buy, qty, price };
                match order_client.place_order(&req).await {
                    Ok(_) => {
                        engine.lock().await.record_buy(&symbol, price, qty);
                    }
                    Err(e) => {
                        tracing::error!(
                            "{} | [{}] | ORDER FAILED {} | error={}",
                            now_local(tz), mode_str, symbol, e
                        );
                    }
                }
            }

            Signal::Exit { price, reason } => {
                let blacklist = matches!(reason, ExitReason::StopLoss);
                let qty = engine.lock().await.get_position_qty(&symbol);

                if qty == 0 {
                    tracing::warn!(
                        "{} | [{}] | EXIT {} ignored — no open position",
                        now_local(tz), mode_str, symbol
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
                                "{} | [{}] | {} {} | price={} | realized_pnl={}{}",
                                now_local(tz), mode_str, reason_str, symbol, price, pnl, blacklist_suffix
                            );
                            let session = engine.lock().await.session_pnl();
                            tracing::info!(
                                "{} | [{}] | SESSION PnL | realized={} | unrealized={} | total={}",
                                now_local(tz), mode_str, session.realized, session.unrealized, session.total()
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                "{} | [{}] | ORDER FAILED {} | error={}",
                                now_local(tz), mode_str, symbol, e
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

fn now_local(tz: chrono_tz::Tz) -> String {
    chrono::Utc::now()
        .with_timezone(&tz)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}
