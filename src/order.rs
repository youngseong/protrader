use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use tokio::sync::Mutex;
use crate::auth::KisAuthProvider;

#[derive(Debug, Clone)]
pub enum OrderSide {
    Buy,
    Sell,
}

#[derive(Debug, Clone)]
pub struct OrderRequest {
    pub symbol: String,
    pub side: OrderSide,
    pub qty: u32,
    pub price: i64,
}

#[async_trait]
pub trait OrderClient: Send + Sync {
    async fn place_order(&self, req: &OrderRequest) -> anyhow::Result<()>;
}

// ── Paper ─────────────────────────────────────────────────────────────────────

struct PaperState {
    balance: i64,
    shares: HashMap<String, u32>,
    avg_cost: HashMap<String, i64>,
    realized_pnl: i64,
}

pub struct PaperOrderClient {
    initial_balance: i64,
    state: Mutex<PaperState>,
}

impl PaperOrderClient {
    pub fn new(initial_balance: i64) -> Self {
        Self {
            initial_balance,
            state: Mutex::new(PaperState {
                balance: initial_balance,
                shares: HashMap::new(),
                avg_cost: HashMap::new(),
                realized_pnl: 0,
            }),
        }
    }
}

#[async_trait]
impl OrderClient for PaperOrderClient {
    async fn place_order(&self, req: &OrderRequest) -> anyhow::Result<()> {
        let mut s = self.state.lock().await;
        match req.side {
            OrderSide::Buy => {
                let cost = req.price * req.qty as i64;
                s.balance -= cost;
                let prev_qty = *s.shares.get(&req.symbol).unwrap_or(&0);
                let prev_cost = *s.avg_cost.get(&req.symbol).unwrap_or(&0);
                let new_avg = if prev_qty == 0 {
                    req.price
                } else {
                    (prev_cost * prev_qty as i64 + req.price * req.qty as i64)
                        / (prev_qty + req.qty) as i64
                };
                let new_qty = prev_qty + req.qty;
                s.shares.insert(req.symbol.clone(), new_qty);
                s.avg_cost.insert(req.symbol.clone(), new_avg);
                tracing::info!(
                    "[PAPER] BUY {} qty={} price={} cost={} | balance={} shares={} avg_cost={}",
                    req.symbol, req.qty, req.price, cost, s.balance, new_qty, new_avg
                );
            }
            OrderSide::Sell => {
                let proceeds = req.price * req.qty as i64;
                let avg = *s.avg_cost.get(&req.symbol).unwrap_or(&req.price);
                let pnl = (req.price - avg) * req.qty as i64;
                s.balance += proceeds;
                s.realized_pnl += pnl;
                let held = s.shares.entry(req.symbol.clone()).or_insert(0);
                *held = held.saturating_sub(req.qty);
                if *held == 0 {
                    s.shares.remove(&req.symbol);
                    s.avg_cost.remove(&req.symbol);
                }
                let total_equity: i64 = s.shares.iter()
                    .map(|(sym, &qty)| s.avg_cost.get(sym).unwrap_or(&0) * qty as i64)
                    .sum();
                let return_pct = s.realized_pnl as f64 / self.initial_balance as f64 * 100.0;
                tracing::info!(
                    "[PAPER] SELL {} qty={} price={} proceeds={} pnl={} | balance={} realized_pnl={} return={:.2}% unrealized_equity={}",
                    req.symbol, req.qty, req.price, proceeds, pnl,
                    s.balance, s.realized_pnl, return_pct, total_equity
                );
            }
        }
        Ok(())
    }
}

// ── Live (KIS) ────────────────────────────────────────────────────────────────

pub struct LiveOrderClient {
    http: reqwest::Client,
    auth: Arc<KisAuthProvider>,
}

impl LiveOrderClient {
    pub fn new(auth: Arc<KisAuthProvider>) -> Self {
        Self {
            http: crate::http_client(),
            auth,
        }
    }
}

#[async_trait]
impl OrderClient for LiveOrderClient {
    async fn place_order(&self, req: &OrderRequest) -> anyhow::Result<()> {
        let token = self.auth.token().await;
        let tr_id = match req.side {
            OrderSide::Buy => "TTTC0802U",
            OrderSide::Sell => "TTTC0801U",
        };
        let resp = self
            .http
            .post(format!(
                "{}/uapi/domestic-stock/v1/trading/order-cash",
                self.auth.base_url()
            ))
            .header("content-type", "application/json; charset=utf-8")
            .header("authorization", format!("Bearer {}", token))
            .header("appkey", self.auth.app_key())
            .header("appsecret", self.auth.app_secret())
            .header("tr_id", tr_id)
            .json(&serde_json::json!({
                "CANO": self.auth.account_no(),
                "ACNT_PRDT_CD": "01",
                "PDNO": req.symbol,
                "ORD_DVSN": "00",
                "ORD_QTY": req.qty.to_string(),
                "ORD_UNPR": "0",
            }))
            .send()
            .await?;
        if !resp.status().is_success() {
            anyhow::bail!("KIS order failed: HTTP {}", resp.status());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_paper_order_always_succeeds() {
        let client = PaperOrderClient::new(1_000_000);
        let req = OrderRequest {
            symbol: "005930".to_string(),
            side: OrderSide::Buy,
            qty: 7,
            price: 71_400,
        };
        client.place_order(&req).await.expect("paper order should not fail");
    }

    #[tokio::test]
    async fn test_paper_tracks_balance_and_pnl() {
        let client = PaperOrderClient::new(10_000_000);

        client.place_order(&OrderRequest {
            symbol: "005930".to_string(),
            side: OrderSide::Buy,
            qty: 10,
            price: 70_000,
        }).await.unwrap();

        client.place_order(&OrderRequest {
            symbol: "005930".to_string(),
            side: OrderSide::Sell,
            qty: 10,
            price: 75_000,
        }).await.unwrap();

        let s = client.state.lock().await;
        assert_eq!(s.balance, 10_000_000 - 700_000 + 750_000); // 10_050_000
        assert_eq!(s.realized_pnl, 50_000);
        assert!(s.shares.is_empty());
    }
}
