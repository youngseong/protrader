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
