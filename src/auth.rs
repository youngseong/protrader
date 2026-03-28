use std::sync::Arc;
use tokio::sync::RwLock;
use crate::config::KisCredentials;

pub struct KisAuthProvider {
    http: reqwest::Client,
    base_url: String,
    credentials: KisCredentials,
    token: RwLock<String>,
}

impl KisAuthProvider {
    /// Fetch the initial token and spawn a background task that refreshes it
    /// every 23 hours (KIS tokens expire after 24 hours).
    pub async fn new(
        http: reqwest::Client,
        base_url: String,
        credentials: KisCredentials,
    ) -> anyhow::Result<Arc<Self>> {
        let token = Self::fetch_token_raw(&http, &base_url, &credentials).await?;
        let provider = Arc::new(Self {
            http,
            base_url,
            credentials,
            token: RwLock::new(token),
        });
        provider.clone().spawn_refresh_task();
        Ok(provider)
    }

    /// Returns a clone of the current bearer token.
    pub async fn token(&self) -> String {
        self.token.read().await.clone()
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn app_key(&self) -> &str {
        &self.credentials.app_key
    }

    pub fn app_secret(&self) -> &str {
        &self.credentials.app_secret
    }

    pub fn account_no(&self) -> &str {
        &self.credentials.account_no
    }

    async fn refresh(&self) -> anyhow::Result<()> {
        let new_token = Self::fetch_token_raw(&self.http, &self.base_url, &self.credentials).await?;
        *self.token.write().await = new_token;
        Ok(())
    }

    fn spawn_refresh_task(self: Arc<Self>) {
        let weak = Arc::downgrade(&self);
        tokio::spawn(async move {
            let interval = tokio::time::Duration::from_secs(23 * 60 * 60);
            loop {
                tokio::time::sleep(interval).await;
                match weak.upgrade() {
                    Some(provider) => {
                        if let Err(e) = provider.refresh().await {
                            tracing::error!("KIS token refresh failed: {}", e);
                        } else {
                            tracing::info!("KIS auth token refreshed");
                        }
                    }
                    None => break,
                }
            }
        });
    }

    async fn fetch_token_raw(
        http: &reqwest::Client,
        base_url: &str,
        creds: &KisCredentials,
    ) -> anyhow::Result<String> {
        #[derive(serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
        }
        let raw = http
            .post(format!("{}/oauth2/tokenP", base_url))
            .json(&serde_json::json!({
                "grant_type": "client_credentials",
                "appkey": creds.app_key,
                "appsecret": creds.app_secret,
            }))
            .send()
            .await?
            .error_for_status()
            .map_err(|e| anyhow::anyhow!("KIS token request failed (HTTP {})", e.status().map_or_else(|| "?".to_string(), |s| s.to_string())))?
            .bytes()
            .await?;
        let resp: TokenResponse = serde_json::from_slice(&raw).map_err(|e| {
            anyhow::anyhow!(
                "KIS token response parse error: {} — body: {}",
                e,
                String::from_utf8_lossy(&raw)
            )
        })?;
        Ok(resp.access_token)
    }
}
