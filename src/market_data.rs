use async_trait::async_trait;
use crate::config::Credentials;

#[async_trait]
pub trait MarketDataClient: Send + Sync {
    async fn fetch_price(&self, symbol: &str) -> anyhow::Result<i64>;
}

// ── KIS HTTP implementation ───────────────────────────────────────────────────

pub struct KisMarketDataClient {
    http: reqwest::Client,
    base_url: String,
    credentials: Credentials,
    token: tokio::sync::RwLock<String>,
}

impl KisMarketDataClient {
    pub async fn new(credentials: Credentials) -> anyhow::Result<Self> {
        let http = reqwest::Client::new();
        let base_url = "https://openapi.koreainvestment.com:9443".to_string();
        let token = Self::fetch_token(&http, &base_url, &credentials).await?;
        Ok(Self {
            http,
            base_url,
            credentials,
            token: tokio::sync::RwLock::new(token),
        })
    }

    async fn fetch_token(
        http: &reqwest::Client,
        base_url: &str,
        creds: &Credentials,
    ) -> anyhow::Result<String> {
        #[derive(serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
        }
        let resp: TokenResponse = http
            .post(format!("{}/oauth2/tokenP", base_url))
            .json(&serde_json::json!({
                "grant_type": "client_credentials",
                "appkey": creds.app_key,
                "appsecret": creds.app_secret,
            }))
            .send()
            .await?
            .json()
            .await?;
        Ok(resp.access_token)
    }
}

#[async_trait]
impl MarketDataClient for KisMarketDataClient {
    async fn fetch_price(&self, symbol: &str) -> anyhow::Result<i64> {
        #[derive(serde::Deserialize)]
        struct PriceOutput {
            stck_prpr: String, // current price as a string in KIS API
        }
        #[derive(serde::Deserialize)]
        struct PriceResponse {
            output: PriceOutput,
        }

        let token = self.token.read().await;
        let resp: PriceResponse = self
            .http
            .get(format!(
                "{}/uapi/domestic-stock/v1/quotations/inquire-price",
                self.base_url
            ))
            .header("content-type", "application/json; charset=utf-8")
            .header("authorization", format!("Bearer {}", *token))
            .header("appkey", &self.credentials.app_key)
            .header("appsecret", &self.credentials.app_secret)
            .header("tr_id", "FHKST01010100")
            .query(&[
                ("FID_COND_MRKT_DIV_CODE", "J"),
                ("FID_INPUT_ISCD", symbol),
            ])
            .send()
            .await?
            .json()
            .await?;

        let price: i64 = resp.output.stck_prpr.trim().parse()?;
        Ok(price)
    }
}

// ── Mock for testing ──────────────────────────────────────────────────────────

/// Returns prices from a pre-loaded sequence per symbol.
/// Repeats the last price once the sequence is exhausted.
pub struct MockMarketDataClient {
    prices: std::collections::HashMap<
        String,
        std::sync::Mutex<std::collections::VecDeque<i64>>,
    >,
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
    async fn fetch_price(&self, symbol: &str) -> anyhow::Result<i64> {
        let mut deque = self
            .prices
            .get(symbol)
            .ok_or_else(|| anyhow::anyhow!("unknown symbol: {}", symbol))?
            .lock()
            .unwrap();
        if deque.len() > 1 {
            Ok(deque.pop_front().unwrap())
        } else {
            Ok(*deque.front().unwrap())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_returns_sequence_then_repeats_last() {
        let mut prices = std::collections::HashMap::new();
        prices.insert("005930".to_string(), vec![71_000, 72_000, 73_000]);
        let client = MockMarketDataClient::new(prices);

        assert_eq!(client.fetch_price("005930").await.unwrap(), 71_000);
        assert_eq!(client.fetch_price("005930").await.unwrap(), 72_000);
        assert_eq!(client.fetch_price("005930").await.unwrap(), 73_000);
        assert_eq!(client.fetch_price("005930").await.unwrap(), 73_000); // repeated
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
        let creds = crate::config::Credentials::from_env();
        let client = KisMarketDataClient::new(creds).await.expect("token fetch failed");

        for symbol in &["005930", "069500"] {
            let price = client.fetch_price(symbol).await.expect("price fetch failed");
            println!("{symbol}: ₩{price}");
            assert!(price > 0);
        }
    }
}
