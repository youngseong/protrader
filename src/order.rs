use std::sync::Arc;
use async_trait::async_trait;
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

pub struct PaperOrderClient;

#[async_trait]
impl OrderClient for PaperOrderClient {
    async fn place_order(&self, _req: &OrderRequest) -> anyhow::Result<()> {
        // Logging is handled by the scheduler; this is intentionally a no-op.
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
            http: reqwest::Client::new(),
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
        let client = PaperOrderClient;
        let req = OrderRequest {
            symbol: "005930".to_string(),
            side: OrderSide::Buy,
            qty: 7,
            price: 71_400,
        };
        client.place_order(&req).await.expect("paper order should not fail");
    }
}
