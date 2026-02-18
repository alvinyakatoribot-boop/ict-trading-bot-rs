pub mod coinbase;
pub mod historical;

pub use coinbase::CoinbaseClient;
pub use historical::HistoricalExchange;

use anyhow::Result;
use async_trait::async_trait;

use crate::models::{CandleSeries, Timeframe};

#[async_trait]
pub trait Exchange: Send + Sync {
    async fn fetch_ohlcv(&mut self, tf: Timeframe, limit: usize) -> Result<CandleSeries>;
    async fn get_current_price(&mut self) -> Result<f64>;
    async fn get_4h(&mut self, limit: usize) -> Result<CandleSeries>;
    async fn get_midnight_open(&mut self) -> Result<Option<f64>>;
}
