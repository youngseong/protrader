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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_env_returns_none_when_vars_missing() {
        unsafe {
            std::env::remove_var("TELEGRAM_BOT_TOKEN");
            std::env::remove_var("TELEGRAM_CHAT_ID");
        }
        assert!(TelegramNotifier::from_env().is_none());
    }

    #[test]
    fn test_from_env_returns_none_when_chat_id_missing() {
        unsafe {
            std::env::set_var("TELEGRAM_BOT_TOKEN", "test_token");
            std::env::remove_var("TELEGRAM_CHAT_ID");
        }
        assert!(TelegramNotifier::from_env().is_none());
        unsafe {
            std::env::remove_var("TELEGRAM_BOT_TOKEN");
        }
    }

    #[test]
    fn test_from_env_returns_some_when_both_vars_set() {
        unsafe {
            std::env::set_var("TELEGRAM_BOT_TOKEN", "test_token");
            std::env::set_var("TELEGRAM_CHAT_ID", "123456");
        }
        assert!(TelegramNotifier::from_env().is_some());
        unsafe {
            std::env::remove_var("TELEGRAM_BOT_TOKEN");
            std::env::remove_var("TELEGRAM_CHAT_ID");
        }
    }

    /// Live smoke test — requires TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID in env.
    /// Run with: cargo test live_telegram -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn test_live_telegram_send() {
        let _ = dotenvy::dotenv();
        let notifier = TelegramNotifier::from_env()
            .expect("TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID must be set");
        notifier
            .send("[PAPER] BUY 005930\nprice=75000 | qty=6 | amount=450000")
            .await;
        println!("Message sent — check your Telegram chat.");
    }
}
