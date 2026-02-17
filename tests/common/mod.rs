use chrono::{DateTime, Duration, Utc};
use ict_trading_bot::models::{Candle, CandleSeries};

/// Create candles from (open, high, low, close) tuples with auto-incrementing 1m timestamps.
pub fn make_candles(data: &[(f64, f64, f64, f64)]) -> CandleSeries {
    let base = DateTime::parse_from_rfc3339("2024-01-15T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    let candles: Vec<Candle> = data
        .iter()
        .enumerate()
        .map(|(i, &(o, h, l, c))| Candle {
            timestamp: base + Duration::minutes(i as i64),
            open: o,
            high: h,
            low: l,
            close: c,
            volume: 100.0,
        })
        .collect();

    CandleSeries::new(candles)
}

/// Create n rising (bullish) candles starting from `start` price.
pub fn make_bullish_trend(n: usize, start: f64) -> CandleSeries {
    let base = DateTime::parse_from_rfc3339("2024-01-15T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    let candles: Vec<Candle> = (0..n)
        .map(|i| {
            let open = start + i as f64 * 10.0;
            let close = open + 8.0;
            Candle {
                timestamp: base + Duration::minutes(i as i64),
                open,
                high: close + 2.0,
                low: open - 1.0,
                close,
                volume: 100.0,
            }
        })
        .collect();

    CandleSeries::new(candles)
}

/// Create n falling (bearish) candles starting from `start` price.
pub fn make_bearish_trend(n: usize, start: f64) -> CandleSeries {
    let base = DateTime::parse_from_rfc3339("2024-01-15T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);

    let candles: Vec<Candle> = (0..n)
        .map(|i| {
            let open = start - i as f64 * 10.0;
            let close = open - 8.0;
            Candle {
                timestamp: base + Duration::minutes(i as i64),
                open,
                high: open + 1.0,
                low: close - 2.0,
                close,
                volume: 100.0,
            }
        })
        .collect();

    CandleSeries::new(candles)
}
