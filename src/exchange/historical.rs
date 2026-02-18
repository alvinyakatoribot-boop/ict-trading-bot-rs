use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Timelike, Utc};
use chrono_tz::US::Eastern;
use std::collections::HashMap;
use std::time::Duration;

use crate::exchange::Exchange;
use crate::models::{Candle, CandleSeries, Timeframe};

/// An Exchange implementation that replays pre-loaded historical data.
/// A cursor (`now`) controls which candles are visible — only candles
/// with timestamp <= now are returned, simulating a forward walk.
pub struct HistoricalExchange {
    data: HashMap<Timeframe, Vec<Candle>>,
    now: DateTime<Utc>,
    symbol: String,
}

impl HistoricalExchange {
    pub fn new(symbol: &str) -> Self {
        Self {
            data: HashMap::new(),
            now: Utc::now(),
            symbol: symbol.to_string(),
        }
    }

    /// Load candles for a specific timeframe.
    /// Candles must be sorted oldest-first.
    pub fn load(&mut self, tf: Timeframe, candles: Vec<Candle>) {
        self.data.insert(tf, candles);
    }

    /// Advance the simulation clock.
    pub fn set_time(&mut self, t: DateTime<Utc>) {
        self.now = t;
    }

    pub fn current_time(&self) -> DateTime<Utc> {
        self.now
    }

    /// Get the earliest timestamp across all loaded timeframes.
    pub fn earliest_time(&self) -> Option<DateTime<Utc>> {
        self.data
            .values()
            .filter_map(|v| v.first().map(|c| c.timestamp))
            .min()
    }

    /// Get the latest timestamp across all loaded timeframes.
    pub fn latest_time(&self) -> Option<DateTime<Utc>> {
        self.data
            .values()
            .filter_map(|v| v.last().map(|c| c.timestamp))
            .max()
    }

    /// Return candles up to `self.now`, capped at `limit`.
    fn visible_candles(&self, tf: Timeframe, limit: usize) -> CandleSeries {
        let empty = Vec::new();
        let all = self.data.get(&tf).unwrap_or(&empty);

        // Binary search for the rightmost candle <= now
        let end = match all.partition_point(|c| c.timestamp <= self.now) {
            0 => return CandleSeries::default(),
            n => n,
        };

        let start = end.saturating_sub(limit);
        CandleSeries::new(all[start..end].to_vec())
    }
}

#[async_trait]
impl Exchange for HistoricalExchange {
    async fn fetch_ohlcv(&mut self, tf: Timeframe, limit: usize) -> Result<CandleSeries> {
        Ok(self.visible_candles(tf, limit))
    }

    async fn get_current_price(&mut self) -> Result<f64> {
        // Use the most recent 1m candle close as current price
        let series = self.visible_candles(Timeframe::M1, 1);
        series
            .last()
            .map(|c| c.close)
            .context("No price data at current time")
    }

    async fn get_4h(&mut self, limit: usize) -> Result<CandleSeries> {
        // Resample from H1 data
        let hours_needed = (limit * 4).min(340);
        let h1 = self.visible_candles(Timeframe::H1, hours_needed);
        Ok(h1.resample(Duration::from_secs(14400)))
    }

    async fn get_midnight_open(&mut self) -> Result<Option<f64>> {
        let h1 = self.visible_candles(Timeframe::H1, 48);
        if h1.is_empty() {
            return Ok(None);
        }

        let today = self.now.with_timezone(&Eastern).date_naive();

        for candle in h1.iter() {
            let candle_et = candle.timestamp.with_timezone(&Eastern);
            if candle_et.date_naive() == today && candle_et.hour() == 0 {
                return Ok(Some(candle.open));
            }
        }

        // Fallback: first candle of today
        for candle in h1.iter() {
            let candle_et = candle.timestamp.with_timezone(&Eastern);
            if candle_et.date_naive() == today {
                return Ok(Some(candle.open));
            }
        }

        Ok(None)
    }
}
