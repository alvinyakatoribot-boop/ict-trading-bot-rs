use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::{BosType, CandleSeries, SwingType, Trend};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwingPoint {
    pub swing_type: SwingType,
    pub price: f64,
    pub timestamp: DateTime<Utc>,
    pub broken: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DealingRange {
    pub high: f64,
    pub low: f64,
    pub equilibrium: f64,
    pub premium_zone: f64,
    pub discount_zone: f64,
}

impl DealingRange {
    pub fn empty() -> Self {
        Self {
            high: 0.0,
            low: 0.0,
            equilibrium: 0.0,
            premium_zone: 0.0,
            discount_zone: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BosEvent {
    pub bos_type: BosType,
    pub level: f64,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct LiquidityLevels {
    pub bsl: Vec<f64>,
    pub ssl: Vec<f64>,
}

pub struct MarketStructure {
    pub swing_lookback: usize,
    pub swing_highs: Vec<SwingPoint>,
    pub swing_lows: Vec<SwingPoint>,
    pub trend: Trend,
    pub bos_events: Vec<BosEvent>,
}

impl MarketStructure {
    pub fn new() -> Self {
        Self::with_lookback(5)
    }

    pub fn with_lookback(swing_lookback: usize) -> Self {
        Self {
            swing_lookback,
            swing_highs: Vec::new(),
            swing_lows: Vec::new(),
            trend: Trend::Neutral,
            bos_events: Vec::new(),
        }
    }

    pub fn analyze(&mut self, candles: &CandleSeries) -> Trend {
        self.swing_highs.clear();
        self.swing_lows.clear();
        self.bos_events.clear();

        self.find_swings(candles);
        self.detect_bos(candles);
        self.determine_trend();

        self.trend
    }

    pub fn get_dealing_range(&self, candles: Option<&CandleSeries>) -> DealingRange {
        if !self.swing_highs.is_empty() && !self.swing_lows.is_empty() {
            let sh = self
                .swing_highs
                .iter()
                .max_by(|a, b| a.price.partial_cmp(&b.price).unwrap())
                .unwrap();
            let sl = self
                .swing_lows
                .iter()
                .min_by(|a, b| a.price.partial_cmp(&b.price).unwrap())
                .unwrap();
            let rng = sh.price - sl.price;
            DealingRange {
                high: sh.price,
                low: sl.price,
                equilibrium: sl.price + rng * 0.5,
                premium_zone: sl.price + rng * 0.75,
                discount_zone: sl.price + rng * 0.25,
            }
        } else if let Some(cs) = candles {
            if cs.is_empty() {
                return DealingRange::empty();
            }
            let sh_price = cs.highs_max();
            let sl_price = cs.lows_min();
            let rng = sh_price - sl_price;
            DealingRange {
                high: sh_price,
                low: sl_price,
                equilibrium: sl_price + rng * 0.5,
                premium_zone: sl_price + rng * 0.75,
                discount_zone: sl_price + rng * 0.25,
            }
        } else {
            DealingRange::empty()
        }
    }

    pub fn get_liquidity_levels(&self) -> LiquidityLevels {
        let mut bsl: Vec<f64> = self
            .swing_highs
            .iter()
            .filter(|s| !s.broken)
            .map(|s| s.price)
            .collect();
        bsl.sort_by(|a, b| b.partial_cmp(a).unwrap());

        let mut ssl: Vec<f64> = self
            .swing_lows
            .iter()
            .filter(|s| !s.broken)
            .map(|s| s.price)
            .collect();
        ssl.sort_by(|a, b| a.partial_cmp(b).unwrap());

        LiquidityLevels { bsl, ssl }
    }

    fn find_swings(&mut self, candles: &CandleSeries) {
        let lb = self.swing_lookback;
        let len = candles.len();
        if len <= lb * 2 {
            return;
        }

        for i in lb..(len - lb) {
            // Swing high: highest high in window
            let mut is_swing_high = true;
            let current_high = candles[i].high;
            for j in (i.saturating_sub(lb))..=(i + lb).min(len - 1) {
                if candles[j].high > current_high {
                    is_swing_high = false;
                    break;
                }
            }
            if is_swing_high {
                self.swing_highs.push(SwingPoint {
                    swing_type: SwingType::High,
                    price: current_high,
                    timestamp: candles[i].timestamp,
                    broken: false,
                });
            }

            // Swing low: lowest low in window
            let mut is_swing_low = true;
            let current_low = candles[i].low;
            for j in (i.saturating_sub(lb))..=(i + lb).min(len - 1) {
                if candles[j].low < current_low {
                    is_swing_low = false;
                    break;
                }
            }
            if is_swing_low {
                self.swing_lows.push(SwingPoint {
                    swing_type: SwingType::Low,
                    price: current_low,
                    timestamp: candles[i].timestamp,
                    broken: false,
                });
            }
        }
    }

    fn detect_bos(&mut self, candles: &CandleSeries) {
        for i in 1..candles.len() {
            let curr_close = candles[i].close;
            let curr_ts = candles[i].timestamp;

            // Bullish BOS: close above most recent unbroken swing high
            let latest_sh = self
                .swing_highs
                .iter_mut()
                .filter(|s| s.timestamp < curr_ts && !s.broken)
                .max_by(|a, b| a.timestamp.cmp(&b.timestamp));

            if let Some(sh) = latest_sh {
                if curr_close > sh.price {
                    let level = sh.price;
                    sh.broken = true;
                    self.bos_events.push(BosEvent {
                        bos_type: BosType::BullishBos,
                        level,
                        timestamp: curr_ts,
                    });
                }
            }

            // Bearish BOS: close below most recent unbroken swing low
            let latest_sl = self
                .swing_lows
                .iter_mut()
                .filter(|s| s.timestamp < curr_ts && !s.broken)
                .max_by(|a, b| a.timestamp.cmp(&b.timestamp));

            if let Some(sl) = latest_sl {
                if curr_close < sl.price {
                    let level = sl.price;
                    sl.broken = true;
                    self.bos_events.push(BosEvent {
                        bos_type: BosType::BearishBos,
                        level,
                        timestamp: curr_ts,
                    });
                }
            }
        }
    }

    fn determine_trend(&mut self) {
        if self.bos_events.is_empty() {
            self.trend = Trend::Neutral;
            return;
        }

        let recent_count = self.bos_events.len().min(3);
        let recent = &self.bos_events[self.bos_events.len() - recent_count..];

        let bullish = recent
            .iter()
            .filter(|e| e.bos_type == BosType::BullishBos)
            .count();
        let bearish = recent
            .iter()
            .filter(|e| e.bos_type == BosType::BearishBos)
            .count();

        self.trend = if bullish > bearish {
            Trend::Bullish
        } else if bearish > bullish {
            Trend::Bearish
        } else {
            Trend::Neutral
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{make_bullish_trend, make_candles};

    #[test]
    fn analyze_bullish_trend() {
        // Build data with clear swing highs and lows for the lookback=5 window.
        // Pattern: flat plateau at peak (6 candles), dip (6 candles), higher plateau (6 candles)...
        // This ensures the peak candle is the highest in its ±5 window.
        let mut data = Vec::new();
        for wave in 0..4 {
            let trough = 100.0 + wave as f64 * 40.0;
            let peak = trough + 30.0;
            // Rising to peak
            for i in 0..6 {
                let v = trough + i as f64 * 5.0;
                data.push((v, v + 1.0, v - 1.0, v + 0.5));
            }
            // Peak plateau (swing high detectable in ±5 window)
            for _ in 0..2 {
                data.push((peak, peak + 1.0, peak - 2.0, peak - 1.0));
            }
            // Pullback
            for i in 0..6 {
                let v = peak - i as f64 * 3.0;
                data.push((v, v + 0.5, v - 1.0, v - 0.5));
            }
        }
        // Final up leg so close breaks above swing highs
        let final_peak = 100.0 + 4.0 * 40.0;
        for i in 0..8 {
            let v = final_peak - 15.0 + i as f64 * 5.0;
            data.push((v, v + 1.0, v - 0.5, v + 0.5));
        }
        let candles = make_candles(&data);
        let mut ms = MarketStructure::new();
        let trend = ms.analyze(&candles);
        // With rising swing highs being broken, should get bullish BOS
        assert!(
            trend == Trend::Bullish,
            "Expected Bullish, got {:?}. SH={}, SL={}, BOS={}",
            trend,
            ms.swing_highs.len(),
            ms.swing_lows.len(),
            ms.bos_events.len()
        );
    }

    #[test]
    fn analyze_bearish_trend() {
        let mut data = Vec::new();
        for wave in 0..4 {
            let peak = 500.0 - wave as f64 * 40.0;
            let trough = peak - 30.0;
            // Falling to trough
            for i in 0..6 {
                let v = peak - i as f64 * 5.0;
                data.push((v, v + 1.0, v - 1.0, v - 0.5));
            }
            // Trough plateau
            for _ in 0..2 {
                data.push((trough, trough + 2.0, trough - 1.0, trough + 1.0));
            }
            // Pullback up
            for i in 0..6 {
                let v = trough + i as f64 * 3.0;
                data.push((v, v + 1.0, v - 0.5, v + 0.5));
            }
        }
        // Final down leg breaking below swing lows
        let final_trough = 500.0 - 4.0 * 40.0;
        for i in 0..8 {
            let v = final_trough + 15.0 - i as f64 * 5.0;
            data.push((v, v + 0.5, v - 1.0, v - 0.5));
        }
        let candles = make_candles(&data);
        let mut ms = MarketStructure::new();
        let trend = ms.analyze(&candles);
        assert!(
            trend == Trend::Bearish,
            "Expected Bearish, got {:?}. SH={}, SL={}, BOS={}",
            trend,
            ms.swing_highs.len(),
            ms.swing_lows.len(),
            ms.bos_events.len()
        );
    }

    #[test]
    fn analyze_neutral_for_flat() {
        // Flat candles: no significant swings
        let data: Vec<(f64, f64, f64, f64)> = (0..20)
            .map(|_| (100.0, 100.5, 99.5, 100.0))
            .collect();
        let candles = make_candles(&data);
        let mut ms = MarketStructure::new();
        let trend = ms.analyze(&candles);
        assert_eq!(trend, Trend::Neutral);
    }

    #[test]
    fn find_swings_detects_peak() {
        // Build a V-shape: rising then falling
        let mut data = Vec::new();
        for i in 0..15 {
            let v = 100.0 + i as f64 * 5.0;
            data.push((v, v + 2.0, v - 1.0, v + 1.0));
        }
        for i in 0..15 {
            let v = 170.0 - i as f64 * 5.0;
            data.push((v, v + 2.0, v - 1.0, v - 1.0));
        }
        let candles = make_candles(&data);
        let mut ms = MarketStructure::new();
        ms.analyze(&candles);
        // Should find at least one swing high near the peak
        assert!(!ms.swing_highs.is_empty());
        let max_sh = ms.swing_highs.iter().map(|s| s.price).fold(f64::NEG_INFINITY, f64::max);
        assert!(max_sh > 150.0);
    }

    #[test]
    fn dealing_range_equilibrium() {
        let candles = make_bullish_trend(30, 100.0);
        let mut ms = MarketStructure::new();
        ms.analyze(&candles);
        let dr = ms.get_dealing_range(Some(&candles));
        assert!(dr.high > dr.low);
        let expected_eq = (dr.high + dr.low) / 2.0;
        assert!((dr.equilibrium - expected_eq).abs() < 0.01);
        assert!(dr.premium_zone > dr.equilibrium);
        assert!(dr.discount_zone < dr.equilibrium);
    }
}
