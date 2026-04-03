use crate::auth::KisAuthProvider;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct Quote {
    pub price: i64,
    pub timestamp: DateTime<Utc>,
    pub volume: Option<u64>,
}

#[async_trait]
pub trait MarketDataClient: Send + Sync {
    async fn fetch_price(&self, symbol: &str) -> anyhow::Result<Quote>;
}

// ── KIS HTTP implementation ───────────────────────────────────────────────────

pub struct KisMarketDataClient {
    http: reqwest::Client,
    auth: Arc<KisAuthProvider>,
}

impl KisMarketDataClient {
    pub fn new(auth: Arc<KisAuthProvider>) -> Self {
        Self {
            http: crate::http_client(),
            auth,
        }
    }
}

#[async_trait]
impl MarketDataClient for KisMarketDataClient {
    async fn fetch_price(&self, symbol: &str) -> anyhow::Result<Quote> {
        #[derive(serde::Deserialize)]
        struct PriceOutput {
            stck_prpr: String,
            acml_vol: Option<String>,
        }
        #[derive(serde::Deserialize)]
        struct PriceResponse {
            output: PriceOutput,
        }

        let token = self.auth.token().await;
        let resp: PriceResponse = self
            .http
            .get(format!(
                "{}/uapi/domestic-stock/v1/quotations/inquire-price",
                self.auth.base_url()
            ))
            .header("content-type", "application/json; charset=utf-8")
            .header("authorization", format!("Bearer {}", token))
            .header("appkey", self.auth.app_key())
            .header("appsecret", self.auth.app_secret())
            .header("tr_id", "FHKST01010100")
            .query(&[("FID_COND_MRKT_DIV_CODE", "J"), ("FID_INPUT_ISCD", symbol)])
            .send()
            .await?
            .json()
            .await?;

        let price: i64 = resp.output.stck_prpr.trim().parse()?;
        let volume = resp
            .output
            .acml_vol
            .as_deref()
            .and_then(|v| v.trim().parse::<u64>().ok());
        Ok(Quote {
            price,
            timestamp: Utc::now(),
            volume,
        })
    }
}

// ── Mock for testing ──────────────────────────────────────────────────────────

/// Returns quotes from a pre-loaded price sequence per symbol.
/// Repeats the last price once the sequence is exhausted.
pub struct MockMarketDataClient {
    prices: std::collections::HashMap<String, std::sync::Mutex<std::collections::VecDeque<i64>>>,
}

impl MockMarketDataClient {
    pub fn new(prices: std::collections::HashMap<String, Vec<i64>>) -> Self {
        Self {
            prices: prices
                .into_iter()
                .map(|(k, v)| (k, std::sync::Mutex::new(v.into())))
                .collect(),
        }
    }
}

#[async_trait]
impl MarketDataClient for MockMarketDataClient {
    async fn fetch_price(&self, symbol: &str) -> anyhow::Result<Quote> {
        let mut deque = self
            .prices
            .get(symbol)
            .ok_or_else(|| anyhow::anyhow!("unknown symbol: {}", symbol))?
            .lock()
            .unwrap();
        let price = if deque.len() > 1 {
            deque.pop_front().unwrap()
        } else {
            *deque.front().unwrap()
        };
        Ok(Quote {
            price,
            timestamp: Utc::now(),
            volume: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_returns_sequence_then_repeats_last() {
        let mut prices = std::collections::HashMap::new();
        let ticker = "005930";

        prices.insert(ticker.to_string(), vec![71_000, 72_000, 73_000]);
        let client = MockMarketDataClient::new(prices);

        assert_eq!(client.fetch_price(ticker).await.unwrap().price, 71_000);
        assert_eq!(client.fetch_price(ticker).await.unwrap().price, 72_000);
        assert_eq!(client.fetch_price(ticker).await.unwrap().price, 73_000);
        assert_eq!(client.fetch_price(ticker).await.unwrap().price, 73_000); // repeated
    }

    #[tokio::test]
    async fn test_mock_unknown_symbol_returns_error() {
        let client = MockMarketDataClient::new(std::collections::HashMap::new());
        assert!(client.fetch_price("unknown").await.is_err());
    }

    /// Live API smoke test — requires KIS_APP_KEY / KIS_APP_SECRET / KIS_ACCOUNT_NO in env.
    /// Run with: cargo test live_price -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn test_live_price_fetch() {
        let _ = dotenvy::dotenv();
        let creds = crate::config::KisCredentials::from_env();
        let http = crate::http_client();
        let base_url = "https://openapi.koreainvestment.com:9443".to_string();
        let auth = crate::auth::KisAuthProvider::new(http, base_url, creds)
            .await
            .expect("token fetch failed");
        let client = KisMarketDataClient::new(auth);

        for symbol in &["005930", "069500"] {
            let quote = client
                .fetch_price(symbol)
                .await
                .expect("price fetch failed");
            println!("{symbol}: ₩{}", quote.price);
            assert!(quote.price > 0);
        }
    }
}
