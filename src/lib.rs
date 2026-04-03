pub mod auth;
pub mod backtest;
pub mod config;
pub mod historical;
pub mod logging;
pub mod market_data;
pub mod order;
pub mod scheduler;
pub mod strategies;
/// Compatibility re-export so existing `crate::strategy::*` paths keep working.
pub use strategies as strategy;
pub mod telegram;

/// Build a shared HTTP client with settings tuned for the KIS API.
///
/// `pool_idle_timeout` is set to 25 s to avoid reusing connections that the
/// KIS server has already closed on its end (which causes "connection closed
/// before message completed" errors on the next request).
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .pool_idle_timeout(std::time::Duration::from_secs(25))
        .tcp_keepalive(std::time::Duration::from_secs(15))
        .build()
        .expect("failed to build HTTP client")
}
