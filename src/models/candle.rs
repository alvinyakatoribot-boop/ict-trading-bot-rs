use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candle {
    pub timestamp: DateTime<Utc>,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

impl Candle {
    pub fn body(&self) -> f64 {
        (self.close - self.open).abs()
    }

    pub fn total_range(&self) -> f64 {
        self.high - self.low
    }

    pub fn upper_wick(&self) -> f64 {
        self.high - self.close.max(self.open)
    }

    pub fn lower_wick(&self) -> f64 {
        self.close.min(self.open) - self.low
    }

    pub fn is_bullish(&self) -> bool {
        self.close > self.open
    }

    pub fn is_bearish(&self) -> bool {
        self.close < self.open
    }

    pub fn body_top(&self) -> f64 {
        self.close.max(self.open)
    }

    pub fn body_bottom(&self) -> f64 {
        self.close.min(self.open)
    }
}

/// Wraps Vec<Candle> with helper methods replacing DataFrame operations.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CandleSeries {
    candles: Vec<Candle>,
}

impl CandleSeries {
    pub fn new(candles: Vec<Candle>) -> Self {
        Self { candles }
    }

    pub fn from_raw(
        timestamps: Vec<DateTime<Utc>>,
        opens: Vec<f64>,
        highs: Vec<f64>,
        lows: Vec<f64>,
        closes: Vec<f64>,
        volumes: Vec<f64>,
    ) -> Self {
        let candles = timestamps
            .into_iter()
            .zip(opens)
            .zip(highs)
            .zip(lows)
            .zip(closes)
            .zip(volumes)
            .map(|(((((ts, o), h), l), c), v)| Candle {
                timestamp: ts,
                open: o,
                high: h,
                low: l,
                close: c,
                volume: v,
            })
            .collect();
        Self { candles }
    }

    pub fn len(&self) -> usize {
        self.candles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.candles.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&Candle> {
        self.candles.get(index)
    }

    pub fn last(&self) -> Option<&Candle> {
        self.candles.last()
    }

    pub fn first(&self) -> Option<&Candle> {
        self.candles.first()
    }

    pub fn tail(&self, n: usize) -> CandleSeries {
        let start = self.candles.len().saturating_sub(n);
        CandleSeries::new(self.candles[start..].to_vec())
    }

    pub fn head(&self, n: usize) -> CandleSeries {
        let end = n.min(self.candles.len());
        CandleSeries::new(self.candles[..end].to_vec())
    }

    pub fn slice(&self, start: usize, end: usize) -> CandleSeries {
        let s = start.min(self.candles.len());
        let e = end.min(self.candles.len());
        CandleSeries::new(self.candles[s..e].to_vec())
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Candle> {
        self.candles.iter()
    }

    pub fn as_slice(&self) -> &[Candle] {
        &self.candles
    }

    pub fn highs_max(&self) -> f64 {
        self.candles
            .iter()
            .map(|c| c.high)
            .fold(f64::NEG_INFINITY, f64::max)
    }

    pub fn lows_min(&self) -> f64 {
        self.candles
            .iter()
            .map(|c| c.low)
            .fold(f64::INFINITY, f64::min)
    }

    pub fn closes(&self) -> Vec<f64> {
        self.candles.iter().map(|c| c.close).collect()
    }

    pub fn highs(&self) -> Vec<f64> {
        self.candles.iter().map(|c| c.high).collect()
    }

    pub fn lows(&self) -> Vec<f64> {
        self.candles.iter().map(|c| c.low).collect()
    }

    /// Index of the candle with the highest high
    pub fn high_idx_max(&self) -> Option<usize> {
        self.candles
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.high.partial_cmp(&b.high).unwrap())
            .map(|(i, _)| i)
    }

    /// Index of the candle with the lowest low
    pub fn low_idx_min(&self) -> Option<usize> {
        self.candles
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.low.partial_cmp(&b.low).unwrap())
            .map(|(i, _)| i)
    }

    /// Check if any candle's low is below the given price
    pub fn any_low_below(&self, price: f64) -> bool {
        self.candles.iter().any(|c| c.low < price)
    }

    /// Check if any candle's high is above the given price
    pub fn any_high_above(&self, price: f64) -> bool {
        self.candles.iter().any(|c| c.high > price)
    }

    /// Check if any candle's close is above the given price
    pub fn any_close_above(&self, price: f64) -> bool {
        self.candles.iter().any(|c| c.close > price)
    }

    /// Check if any candle's close is below the given price
    pub fn any_close_below(&self, price: f64) -> bool {
        self.candles.iter().any(|c| c.close < price)
    }

    /// Resample to a larger timeframe bucket
    pub fn resample(&self, bucket: Duration) -> CandleSeries {
        if self.candles.is_empty() {
            return CandleSeries::default();
        }
        let bucket_secs = bucket.as_secs() as i64;
        let mut result: Vec<Candle> = Vec::new();

        for candle in &self.candles {
            let ts = candle.timestamp.timestamp();
            let bucket_start = ts - (ts % bucket_secs);
            let bucket_ts =
                DateTime::from_timestamp(bucket_start, 0).unwrap_or(candle.timestamp);

            if let Some(last) = result.last_mut() {
                if last.timestamp == bucket_ts {
                    last.high = last.high.max(candle.high);
                    last.low = last.low.min(candle.low);
                    last.close = candle.close;
                    last.volume += candle.volume;
                    continue;
                }
            }

            result.push(Candle {
                timestamp: bucket_ts,
                open: candle.open,
                high: candle.high,
                low: candle.low,
                close: candle.close,
                volume: candle.volume,
            });
        }

        CandleSeries::new(result)
    }

    /// Filter candles by date (for daily grouping)
    pub fn filter_by_date(&self, date: chrono::NaiveDate) -> CandleSeries {
        let candles: Vec<Candle> = self
            .candles
            .iter()
            .filter(|c| c.timestamp.date_naive() == date)
            .cloned()
            .collect();
        CandleSeries::new(candles)
    }

    /// Get candles at or after a given timestamp
    pub fn since(&self, ts: DateTime<Utc>) -> CandleSeries {
        let candles: Vec<Candle> = self
            .candles
            .iter()
            .filter(|c| c.timestamp >= ts)
            .cloned()
            .collect();
        CandleSeries::new(candles)
    }

    pub fn push(&mut self, candle: Candle) {
        self.candles.push(candle);
    }
}

impl std::ops::Index<usize> for CandleSeries {
    type Output = Candle;
    fn index(&self, index: usize) -> &Self::Output {
        &self.candles[index]
    }
}

impl IntoIterator for CandleSeries {
    type Item = Candle;
    type IntoIter = std::vec::IntoIter<Candle>;
    fn into_iter(self) -> Self::IntoIter {
        self.candles.into_iter()
    }
}

impl<'a> IntoIterator for &'a CandleSeries {
    type Item = &'a Candle;
    type IntoIter = std::slice::Iter<'a, Candle>;
    fn into_iter(self) -> Self::IntoIter {
        self.candles.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::make_candles;
    use chrono::NaiveDate;

    fn bullish_candle() -> Candle {
        Candle {
            timestamp: Utc::now(),
            open: 100.0,
            high: 115.0,
            low: 95.0,
            close: 110.0,
            volume: 50.0,
        }
    }

    fn bearish_candle() -> Candle {
        Candle {
            timestamp: Utc::now(),
            open: 110.0,
            high: 115.0,
            low: 95.0,
            close: 100.0,
            volume: 50.0,
        }
    }

    #[test]
    fn candle_body_and_range() {
        let c = bullish_candle();
        assert!((c.body() - 10.0).abs() < 1e-9);
        assert!((c.total_range() - 20.0).abs() < 1e-9);
    }

    #[test]
    fn candle_wicks() {
        let c = bullish_candle(); // O=100, H=115, L=95, C=110
        assert!((c.upper_wick() - 5.0).abs() < 1e-9);  // 115 - 110
        assert!((c.lower_wick() - 5.0).abs() < 1e-9);  // 100 - 95
    }

    #[test]
    fn candle_bullish_bearish() {
        assert!(bullish_candle().is_bullish());
        assert!(!bullish_candle().is_bearish());
        assert!(bearish_candle().is_bearish());
        assert!(!bearish_candle().is_bullish());
    }

    #[test]
    fn candle_body_top_bottom() {
        let b = bullish_candle();
        assert!((b.body_top() - 110.0).abs() < 1e-9);
        assert!((b.body_bottom() - 100.0).abs() < 1e-9);
        let br = bearish_candle();
        assert!((br.body_top() - 110.0).abs() < 1e-9);
        assert!((br.body_bottom() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn series_len_empty_tail_head_slice() {
        let s = make_candles(&[
            (100.0, 105.0, 95.0, 102.0),
            (102.0, 108.0, 100.0, 106.0),
            (106.0, 112.0, 104.0, 110.0),
        ]);
        assert_eq!(s.len(), 3);
        assert!(!s.is_empty());

        let tail = s.tail(2);
        assert_eq!(tail.len(), 2);
        assert!((tail[0].open - 102.0).abs() < 1e-9);

        let head = s.head(1);
        assert_eq!(head.len(), 1);
        assert!((head[0].open - 100.0).abs() < 1e-9);

        let slice = s.slice(1, 3);
        assert_eq!(slice.len(), 2);
    }

    #[test]
    fn series_highs_max_lows_min() {
        let s = make_candles(&[
            (100.0, 200.0, 50.0, 150.0),
            (150.0, 300.0, 80.0, 250.0),
            (250.0, 280.0, 60.0, 270.0),
        ]);
        assert!((s.highs_max() - 300.0).abs() < 1e-9);
        assert!((s.lows_min() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn series_resample_1m_to_5m() {
        // Create 10 one-minute candles; resample to 5m should yield 2 buckets
        let data: Vec<(f64, f64, f64, f64)> = (0..10)
            .map(|i| {
                let v = 100.0 + i as f64;
                (v, v + 2.0, v - 1.0, v + 1.0)
            })
            .collect();
        let s = make_candles(&data);
        let resampled = s.resample(std::time::Duration::from_secs(300));
        // Timestamps start at 12:00:00 UTC.  5-minute buckets: [12:00, 12:05)
        // 10 candles at 1-min intervals = 12:00..12:09, so 2 buckets
        assert_eq!(resampled.len(), 2);
        // first bucket open = first candle open
        assert!((resampled[0].open - 100.0).abs() < 1e-9);
        // first bucket close = 5th candle close (index 4)
        assert!((resampled[0].close - 105.0).abs() < 1e-9);
    }

    #[test]
    fn series_filter_by_date() {
        let base = DateTime::parse_from_rfc3339("2024-03-10T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let candles = vec![
            Candle {
                timestamp: base,
                open: 100.0,
                high: 105.0,
                low: 95.0,
                close: 102.0,
                volume: 10.0,
            },
            Candle {
                timestamp: base + chrono::Duration::days(1),
                open: 102.0,
                high: 110.0,
                low: 100.0,
                close: 108.0,
                volume: 10.0,
            },
        ];
        let s = CandleSeries::new(candles);
        let filtered = s.filter_by_date(NaiveDate::from_ymd_opt(2024, 3, 10).unwrap());
        assert_eq!(filtered.len(), 1);
        assert!((filtered[0].open - 100.0).abs() < 1e-9);
    }
}
