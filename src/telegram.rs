pub struct TelegramNotifier {
    http: reqwest::Client,
    token: String,
    chat_id: String,
}

impl TelegramNotifier {
    /// Returns `None` if `TELEGRAM_BOT_TOKEN` or `TELEGRAM_CHAT_ID` are not set.
    pub fn from_env() -> Option<Self> {
        let token = std::env::var("TELEGRAM_BOT_TOKEN").ok()?;
        let chat_id = std::env::var("TELEGRAM_CHAT_ID").ok()?;
        Some(Self {
            http: reqwest::Client::new(),
            token,
            chat_id,
        })
    }

    /// Fire-and-forget — logs a warning on failure, never panics.
    pub async fn send(&self, text: &str) {
        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.token);
        let result = self
            .http
            .post(&url)
            .json(&serde_json::json!({
                "chat_id": self.chat_id,
                "text": text,
            }))
            .send()
            .await;
        if let Err(e) = result {
            tracing::warn!("Telegram notification failed: {}", e);
        }
    }
}
