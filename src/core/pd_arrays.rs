use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::{CandleSeries, PdaType, Timeframe, Trend, Zone};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pda {
    pub pda_type: PdaType,
    pub direction: Trend, // Bullish or Bearish (not Neutral)
    pub zone: Zone,
    pub high: f64,
    pub low: f64,
    pub midpoint: f64,
    pub timestamp: DateTime<Utc>,
    pub timeframe: Timeframe,
    pub strength: f64,
}

pub struct PdArrayDetector {
    pub detected: Vec<Pda>,
}

impl PdArrayDetector {
    pub fn new() -> Self {
        Self {
            detected: Vec::new(),
        }
    }

    pub fn detect_all(
        &mut self,
        candles: &CandleSeries,
        timeframe: Timeframe,
        fvg_min_gap_percent: f64,
        ob_lookback: usize,
        breaker_lookback: usize,
    ) -> &[Pda] {
        self.detected.clear();
        let eq = Self::equilibrium(candles);

        self.detect_order_blocks(candles, timeframe, eq, ob_lookback);
        self.detect_fvg(candles, timeframe, eq, fvg_min_gap_percent);
        self.detect_breaker_blocks(candles, timeframe, eq, breaker_lookback);
        self.detect_rejection_blocks(candles, timeframe, eq);

        &self.detected
    }

    pub fn get_premium_pdas(&self) -> Vec<&Pda> {
        self.detected.iter().filter(|p| p.zone == Zone::Premium).collect()
    }

    pub fn get_discount_pdas(&self) -> Vec<&Pda> {
        self.detected.iter().filter(|p| p.zone == Zone::Discount).collect()
    }

    pub fn get_by_type(&self, pda_type: PdaType) -> Vec<&Pda> {
        self.detected.iter().filter(|p| p.pda_type == pda_type).collect()
    }

    pub fn get_nearest_pda(&self, price: f64, direction: Trend) -> Option<&Pda> {
        let candidates: Vec<&Pda> = self
            .detected
            .iter()
            .filter(|p| p.direction == direction)
            .collect();

        if candidates.is_empty() {
            return None;
        }

        match direction {
            Trend::Bullish => {
                let below: Vec<&&Pda> = candidates.iter().filter(|p| p.high <= price).collect();
                below
                    .into_iter()
                    .min_by(|a, b| {
                        (price - a.high)
                            .partial_cmp(&(price - b.high))
                            .unwrap()
                    })
                    .copied()
            }
            Trend::Bearish => {
                let above: Vec<&&Pda> = candidates.iter().filter(|p| p.low >= price).collect();
                above
                    .into_iter()
                    .min_by(|a, b| {
                        (a.low - price)
                            .partial_cmp(&(b.low - price))
                            .unwrap()
                    })
                    .copied()
            }
            Trend::Neutral => None,
        }
    }

    fn equilibrium(candles: &CandleSeries) -> f64 {
        let swing_high = candles.highs_max();
        let swing_low = candles.lows_min();
        (swing_high + swing_low) / 2.0
    }

    fn classify_zone(level: f64, eq: f64) -> Zone {
        if level > eq {
            Zone::Premium
        } else {
            Zone::Discount
        }
    }

    fn detect_order_blocks(
        &mut self,
        candles: &CandleSeries,
        tf: Timeframe,
        eq: f64,
        ob_lookback: usize,
    ) {
        let len = candles.len();
        let lookback = ob_lookback.min(len.saturating_sub(2));

        for i in 2..=(lookback + 1) {
            let idx = len.checked_sub(i);
            let idx = match idx {
                Some(v) if v >= 1 => v,
                _ => break,
            };

            let curr = &candles[idx];
            let prev = &candles[idx - 1];
            if idx + 1 >= len {
                continue;
            }

            // Bullish OB: last down candle before strong up move
            if prev.close < prev.open && curr.close > curr.open && curr.close > prev.high {
                let ob_high = prev.high;
                let ob_low = prev.low;
                let mid = (ob_high + ob_low) / 2.0;
                let strength = ((curr.close - prev.high) / prev.high).abs().min(1.0);
                self.detected.push(Pda {
                    pda_type: PdaType::OB,
                    direction: Trend::Bullish,
                    zone: Self::classify_zone(mid, eq),
                    high: ob_high,
                    low: ob_low,
                    midpoint: mid,
                    timestamp: candles[idx - 1].timestamp,
                    timeframe: tf,
                    strength,
                });
            }

            // Bearish OB: last up candle before strong down move
            if prev.close > prev.open && curr.close < curr.open && curr.close < prev.low {
                let ob_high = prev.high;
                let ob_low = prev.low;
                let mid = (ob_high + ob_low) / 2.0;
                let strength = ((prev.low - curr.close) / prev.low).abs().min(1.0);
                self.detected.push(Pda {
                    pda_type: PdaType::OB,
                    direction: Trend::Bearish,
                    zone: Self::classify_zone(mid, eq),
                    high: ob_high,
                    low: ob_low,
                    midpoint: mid,
                    timestamp: candles[idx - 1].timestamp,
                    timeframe: tf,
                    strength,
                });
            }
        }
    }

    fn detect_fvg(
        &mut self,
        candles: &CandleSeries,
        tf: Timeframe,
        eq: f64,
        min_gap_pct: f64,
    ) {
        for i in 2..candles.len() {
            let c1 = &candles[i - 2];
            let c3 = &candles[i];

            // Bullish FVG
            let gap_up = c3.low - c1.high;
            if gap_up > 0.0 {
                let gap_pct = gap_up / c1.high;
                if gap_pct >= min_gap_pct {
                    let mid = (c1.high + c3.low) / 2.0;
                    self.detected.push(Pda {
                        pda_type: PdaType::FVG,
                        direction: Trend::Bullish,
                        zone: Self::classify_zone(mid, eq),
                        high: c3.low,
                        low: c1.high,
                        midpoint: mid,
                        timestamp: candles[i - 1].timestamp,
                        timeframe: tf,
                        strength: (gap_pct * 100.0).min(1.0),
                    });
                }
            }

            // Bearish FVG
            let gap_down = c1.low - c3.high;
            if gap_down > 0.0 {
                let gap_pct = gap_down / c1.low;
                if gap_pct >= min_gap_pct {
                    let mid = (c3.high + c1.low) / 2.0;
                    self.detected.push(Pda {
                        pda_type: PdaType::FVG,
                        direction: Trend::Bearish,
                        zone: Self::classify_zone(mid, eq),
                        high: c1.low,
                        low: c3.high,
                        midpoint: mid,
                        timestamp: candles[i - 1].timestamp,
                        timeframe: tf,
                        strength: (gap_pct * 100.0).min(1.0),
                    });
                }
            }
        }
    }

    fn detect_breaker_blocks(
        &mut self,
        candles: &CandleSeries,
        tf: Timeframe,
        eq: f64,
        breaker_lookback: usize,
    ) {
        let len = candles.len();
        let lookback = breaker_lookback.min(len.saturating_sub(3));

        for i in 3..=(lookback + 2) {
            let idx = match len.checked_sub(i) {
                Some(v) if v >= 1 => v,
                _ => break,
            };

            let c = &candles[idx];
            let subsequent = candles.slice(idx + 1, len);

            // Bullish breaker: was an up candle OB that failed
            if c.close > c.open {
                let broke_below = subsequent.any_low_below(c.low);
                if broke_below {
                    let came_back = subsequent.any_close_above(c.high);
                    if came_back {
                        let mid = (c.high + c.low) / 2.0;
                        self.detected.push(Pda {
                            pda_type: PdaType::BRK,
                            direction: Trend::Bullish,
                            zone: Self::classify_zone(mid, eq),
                            high: c.high,
                            low: c.low,
                            midpoint: mid,
                            timestamp: candles[idx].timestamp,
                            timeframe: tf,
                            strength: 0.7,
                        });
                    }
                }
            }

            // Bearish breaker: was a down candle OB that failed
            if c.close < c.open {
                let broke_above = subsequent.any_high_above(c.high);
                if broke_above {
                    let came_back = subsequent.any_close_below(c.low);
                    if came_back {
                        let mid = (c.high + c.low) / 2.0;
                        self.detected.push(Pda {
                            pda_type: PdaType::BRK,
                            direction: Trend::Bearish,
                            zone: Self::classify_zone(mid, eq),
                            high: c.high,
                            low: c.low,
                            midpoint: mid,
                            timestamp: candles[idx].timestamp,
                            timeframe: tf,
                            strength: 0.7,
                        });
                    }
                }
            }
        }
    }

    fn detect_rejection_blocks(&mut self, candles: &CandleSeries, tf: Timeframe, eq: f64) {
        for i in 0..candles.len() {
            let c = &candles[i];
            let body = c.body();
            let total_range = c.total_range();
            if total_range == 0.0 {
                continue;
            }

            let upper_wick = c.upper_wick();
            let lower_wick = c.lower_wick();

            // Bullish RB: large lower wick (>60%), small body (<30%)
            if lower_wick / total_range > 0.6 && body / total_range < 0.3 {
                let zone_low = c.low;
                let zone_high = c.body_bottom();
                let mid = (zone_high + zone_low) / 2.0;
                self.detected.push(Pda {
                    pda_type: PdaType::RB,
                    direction: Trend::Bullish,
                    zone: Self::classify_zone(mid, eq),
                    high: zone_high,
                    low: zone_low,
                    midpoint: mid,
                    timestamp: c.timestamp,
                    timeframe: tf,
                    strength: lower_wick / total_range,
                });
            }

            // Bearish RB: large upper wick
            if upper_wick / total_range > 0.6 && body / total_range < 0.3 {
                let zone_high = c.high;
                let zone_low = c.body_top();
                let mid = (zone_high + zone_low) / 2.0;
                self.detected.push(Pda {
                    pda_type: PdaType::RB,
                    direction: Trend::Bearish,
                    zone: Self::classify_zone(mid, eq),
                    high: zone_high,
                    low: zone_low,
                    midpoint: mid,
                    timestamp: c.timestamp,
                    timeframe: tf,
                    strength: upper_wick / total_range,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::make_candles;

    fn detect(data: &[(f64, f64, f64, f64)]) -> Vec<Pda> {
        let candles = make_candles(data);
        let mut det = PdArrayDetector::new();
        det.detect_all(&candles, Timeframe::M1, 0.0005, 20, 30);
        det.detected.clone()
    }

    #[test]
    fn detect_bullish_ob() {
        // Need enough candles so the loop can find the OB pattern
        // OB detection: prev is down candle, curr is up candle closing above prev.high
        let mut data = Vec::new();
        // Pad with neutral candles
        for _ in 0..5 {
            data.push((100.0, 101.0, 99.0, 100.0));
        }
        data.push((105.0, 106.0, 98.0, 99.0));   // down candle (OB body) — prev
        data.push((99.0, 115.0, 98.0, 113.0));    // strong up, close > prev.high — curr
        // More padding
        for _ in 0..3 {
            data.push((113.0, 114.0, 112.0, 113.5));
        }
        let pdas = detect(&data);
        let obs: Vec<&Pda> = pdas.iter().filter(|p| p.pda_type == PdaType::OB && p.direction == Trend::Bullish).collect();
        assert!(!obs.is_empty(), "Expected bullish OB, got: {:?}", pdas);
    }

    #[test]
    fn detect_bearish_ob() {
        let mut data = Vec::new();
        for _ in 0..5 {
            data.push((100.0, 101.0, 99.0, 100.0));
        }
        data.push((99.0, 106.0, 98.0, 105.0));   // up candle — prev
        data.push((105.0, 106.0, 88.0, 90.0));    // strong down, close < prev.low — curr
        for _ in 0..3 {
            data.push((90.0, 91.0, 89.0, 90.5));
        }
        let pdas = detect(&data);
        let obs: Vec<&Pda> = pdas.iter().filter(|p| p.pda_type == PdaType::OB && p.direction == Trend::Bearish).collect();
        assert!(!obs.is_empty(), "Expected bearish OB, got: {:?}", pdas);
    }

    #[test]
    fn detect_bullish_fvg() {
        // gap: candle[0].high < candle[2].low
        let data = vec![
            (100.0, 102.0, 98.0, 101.0),
            (103.0, 106.0, 102.5, 105.0),  // middle candle
            (107.0, 110.0, 106.0, 109.0),  // candle[2].low=106 > candle[0].high=102 => gap = 4
        ];
        let pdas = detect(&data);
        let fvgs: Vec<&Pda> = pdas.iter().filter(|p| p.pda_type == PdaType::FVG && p.direction == Trend::Bullish).collect();
        assert!(!fvgs.is_empty(), "Expected bullish FVG, got: {:?}", pdas);
    }

    #[test]
    fn detect_bearish_fvg() {
        // gap: candle[0].low > candle[2].high
        let data = vec![
            (110.0, 115.0, 108.0, 112.0),
            (106.0, 107.0, 103.0, 104.0),
            (100.0, 102.0, 96.0, 98.0),  // candle[2].high=102 < candle[0].low=108 => gap
        ];
        let pdas = detect(&data);
        let fvgs: Vec<&Pda> = pdas.iter().filter(|p| p.pda_type == PdaType::FVG && p.direction == Trend::Bearish).collect();
        assert!(!fvgs.is_empty(), "Expected bearish FVG, got: {:?}", pdas);
    }

    #[test]
    fn no_fvg_when_gap_below_threshold() {
        // gap exists but is smaller than 0.05% of price
        let data = vec![
            (10000.0, 10001.0, 9999.0, 10000.5),
            (10001.0, 10002.0, 10000.5, 10001.5),
            (10001.5, 10003.0, 10001.1, 10002.0),  // gap = 10001.1 - 10001.0 = 0.1, pct = 0.001%
        ];
        let pdas = detect(&data);
        let fvgs: Vec<&Pda> = pdas.iter().filter(|p| p.pda_type == PdaType::FVG).collect();
        assert!(fvgs.is_empty(), "Expected no FVG for tiny gap");
    }

    #[test]
    fn detect_bullish_rejection_block() {
        // Pin bar with large lower wick (>60%) and small body (<30%)
        // O=101, H=102, L=90, C=101.5 => range=12, lower_wick=11, body=0.5
        let data = vec![
            (100.0, 105.0, 95.0, 100.0),
            (101.0, 102.0, 90.0, 101.5),
        ];
        let pdas = detect(&data);
        let rbs: Vec<&Pda> = pdas.iter().filter(|p| p.pda_type == PdaType::RB && p.direction == Trend::Bullish).collect();
        assert!(!rbs.is_empty(), "Expected bullish RB, got: {:?}", pdas);
    }

    #[test]
    fn detect_bearish_rejection_block() {
        // Pin bar with large upper wick (>60%) and small body (<30%)
        // O=99, H=112, L=98, C=99.5 => range=14, upper_wick=12.5, body=0.5
        let data = vec![
            (100.0, 105.0, 95.0, 100.0),
            (99.0, 112.0, 98.0, 99.5),
        ];
        let pdas = detect(&data);
        let rbs: Vec<&Pda> = pdas.iter().filter(|p| p.pda_type == PdaType::RB && p.direction == Trend::Bearish).collect();
        assert!(!rbs.is_empty(), "Expected bearish RB, got: {:?}", pdas);
    }
}
