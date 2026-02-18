use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;

use crate::config::{Config, DayRatings, HftScaleConfig, SessionTime};
use crate::models::{Candle, CandleSeries, Timeframe};

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

/// A Config suitable for testing — paper mode, no API keys needed, temp log dir.
pub fn default_test_config() -> Config {
    let mut sessions = HashMap::new();
    sessions.insert(
        "asian".to_string(),
        SessionTime {
            start: (20, 0),
            end: (0, 0),
        },
    );
    sessions.insert(
        "london".to_string(),
        SessionTime {
            start: (2, 0),
            end: (5, 0),
        },
    );
    sessions.insert(
        "ny_forex".to_string(),
        SessionTime {
            start: (7, 0),
            end: (10, 0),
        },
    );
    sessions.insert(
        "ny_indices".to_string(),
        SessionTime {
            start: (8, 30),
            end: (12, 0),
        },
    );

    let mut session_weights = HashMap::new();
    session_weights.insert("london".to_string(), 1.5);
    session_weights.insert("ny_forex".to_string(), 1.5);
    session_weights.insert("ny_indices".to_string(), 1.3);
    session_weights.insert("asian".to_string(), 0.3);
    session_weights.insert("off_session".to_string(), 0.3);

    let mut hft_scales = HashMap::new();
    hft_scales.insert(
        "1m".to_string(),
        HftScaleConfig {
            name: "1m Scalp".to_string(),
            entry_tf: Timeframe::M1,
            alignment_tfs: vec![Timeframe::M5, Timeframe::M15, Timeframe::H1],
            structure_tf: Timeframe::M5,
            confirm_tf: Timeframe::M1,
            scan_interval: 10,
            min_confidence: 0.5,
            weight: 0.7,
        },
    );
    hft_scales.insert(
        "5m".to_string(),
        HftScaleConfig {
            name: "5m Intraday".to_string(),
            entry_tf: Timeframe::M5,
            alignment_tfs: vec![Timeframe::M15, Timeframe::H1, Timeframe::H4],
            structure_tf: Timeframe::M15,
            confirm_tf: Timeframe::M5,
            scan_interval: 30,
            min_confidence: 0.45,
            weight: 0.85,
        },
    );
    hft_scales.insert(
        "15m".to_string(),
        HftScaleConfig {
            name: "15m Swing".to_string(),
            entry_tf: Timeframe::M15,
            alignment_tfs: vec![Timeframe::H1, Timeframe::H4, Timeframe::D1],
            structure_tf: Timeframe::H1,
            confirm_tf: Timeframe::M15,
            scan_interval: 60,
            min_confidence: 0.4,
            weight: 1.0,
        },
    );

    let mut day_ratings = HashMap::new();
    day_ratings.insert(
        "classic_expansion".to_string(),
        DayRatings {
            monday: 0.0,
            tuesday: 4.0,
            wednesday: 5.0,
            thursday: 4.5,
            friday: 3.5,
            saturday: 3.0,
            sunday: 3.0,
        },
    );
    day_ratings.insert(
        "midweek_reversal".to_string(),
        DayRatings {
            monday: 0.0,
            tuesday: 3.0,
            wednesday: 3.5,
            thursday: 5.0,
            friday: 4.5,
            saturday: 3.0,
            sunday: 3.0,
        },
    );
    day_ratings.insert(
        "consolidation_reversal".to_string(),
        DayRatings {
            monday: 0.0,
            tuesday: 2.0,
            wednesday: 2.5,
            thursday: 4.0,
            friday: 5.0,
            saturday: 3.0,
            sunday: 3.0,
        },
    );
    day_ratings.insert(
        "undetermined".to_string(),
        DayRatings {
            monday: 0.0,
            tuesday: 3.0,
            wednesday: 3.5,
            thursday: 3.5,
            friday: 3.0,
            saturday: 3.0,
            sunday: 3.0,
        },
    );

    Config {
        exchange: "coinbase".to_string(),
        symbol: "BTC-USD".to_string(),
        coinbase_api_key: String::new(),
        coinbase_api_secret: String::new(),
        paper_trade: true,
        initial_balance: 200.0,
        max_daily_loss: 0.03,
        max_open_positions: 3,
        fee_rate: 0.0,
        slippage_rate: 0.0,
        sessions,
        session_weights,
        hft_scales,
        cross_scale_confluence_bonus: 0.1,
        day_ratings,
        min_day_rating: 3.0,
        fvg_min_gap_percent: 0.0005,
        ob_lookback: 20,
        breaker_lookback: 30,
        tgif_retrace_min: 0.20,
        tgif_retrace_max: 0.30,
        analysis_interval: 3600,
        min_sample_per_bucket: 10,
        adjustment_step: 0.02,
        log_dir: std::env::temp_dir()
            .join("ict_bot_test")
            .to_string_lossy()
            .to_string(),
        log_level: "ERROR".to_string(),
    }
}
