use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::pd_arrays::Pda;
use crate::models::{CandleSeries, Direction, StopMode, SwingType, Trend};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtectedSwing {
    pub swing_type: SwingType,
    pub extreme: f64,
    pub body_level: f64,
    pub timestamp: DateTime<Utc>,
    pub sweep_confirmed: bool,
    pub close_confirmed: bool,
    pub strength: f64,
    pub candle_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopLossLevel {
    pub price: f64,
    pub mode: StopMode,
    pub protected_swing: ProtectedSwing,
    pub risk_distance: f64,
    pub risk_percent: f64,
    pub reason: String,
}

const MAX_WICK_RATIO_FOR_BODY: f64 = 0.4;
const MIN_RR_THRESHOLD: f64 = 1.5;

pub struct StopLossEngine {
    pub swing_lookback: usize,
    pub protected_swings: Vec<ProtectedSwing>,
}

impl StopLossEngine {
    pub fn new() -> Self {
        Self::with_lookback(3)
    }

    pub fn with_lookback(lookback: usize) -> Self {
        Self {
            swing_lookback: lookback,
            protected_swings: Vec::new(),
        }
    }

    pub fn find_protected_swings(
        &mut self,
        candles: &CandleSeries,
        pdas: Option<&[Pda]>,
    ) -> &[ProtectedSwing] {
        self.protected_swings.clear();
        if candles.len() < self.swing_lookback + 2 {
            return &self.protected_swings;
        }

        let (swing_highs, swing_lows) = self.find_raw_swings(candles);

        for sh in &swing_highs {
            if let Some(ps) = self.validate_protected_swing(candles, sh, SwingType::High, pdas) {
                self.protected_swings.push(ps);
            }
        }

        for sl in &swing_lows {
            if let Some(ps) = self.validate_protected_swing(candles, sl, SwingType::Low, pdas) {
                self.protected_swings.push(ps);
            }
        }

        self.protected_swings
            .sort_by(|a, b| b.timestamp.cmp(&a.timestamp));

        &self.protected_swings
    }

    pub fn get_stop_loss(
        &mut self,
        entry_price: f64,
        direction: Direction,
        take_profit: f64,
        candles: &CandleSeries,
        pdas: Option<&[Pda]>,
    ) -> StopLossLevel {
        if self.protected_swings.is_empty() {
            self.find_protected_swings(candles, pdas);
        }

        let swing = self.get_nearest_protected_swing(entry_price, direction);

        let swing = match swing {
            Some(s) => s.clone(),
            None => return self.fallback_stop(entry_price, direction, candles),
        };

        let reward_distance = (take_profit - entry_price).abs();
        let swing_type = swing.swing_type;

        // Mode 1: WICK
        let wick_stop = swing.extreme;
        let wick_distance = (entry_price - wick_stop).abs();
        let wick_rr = if wick_distance > 0.0 {
            reward_distance / wick_distance
        } else {
            0.0
        };

        if wick_rr >= MIN_RR_THRESHOLD {
            let reason = format!(
                "Protected swing {} (wick) @ {:.2} | R:R {:.1}",
                swing_type, wick_stop, wick_rr
            );
            return StopLossLevel {
                price: round2(wick_stop),
                mode: StopMode::Wick,
                protected_swing: swing,
                risk_distance: round2(wick_distance),
                risk_percent: round3(wick_distance / entry_price * 100.0),
                reason,
            };
        }

        // Mode 2: BODY
        let body_stop = swing.body_level;
        let body_safe = self.is_body_mode_safe(&swing, candles);
        let body_distance = (entry_price - body_stop).abs();
        let body_rr = if body_distance > 0.0 {
            reward_distance / body_distance
        } else {
            0.0
        };

        if body_safe && body_rr >= MIN_RR_THRESHOLD {
            let reason = format!(
                "Protected swing {} (body) @ {:.2} | R:R {:.1}",
                swing_type, body_stop, body_rr
            );
            return StopLossLevel {
                price: round2(body_stop),
                mode: StopMode::Body,
                protected_swing: swing,
                risk_distance: round2(body_distance),
                risk_percent: round3(body_distance / entry_price * 100.0),
                reason,
            };
        }

        // Mode 3: CONTINUATION
        if let Some(continuation) = self.find_continuation_swing(entry_price, direction, &swing) {
            let cont_stop = continuation.extreme;
            let cont_distance = (entry_price - cont_stop).abs();
            let cont_rr = if cont_distance > 0.0 {
                reward_distance / cont_distance
            } else {
                0.0
            };

            if cont_rr >= MIN_RR_THRESHOLD {
                let reason = format!(
                    "Continuation swing {} @ {:.2} | R:R {:.1} (tighter than original {:.2})",
                    swing_type, cont_stop, cont_rr, wick_stop
                );
                return StopLossLevel {
                    price: round2(cont_stop),
                    mode: StopMode::Continuation,
                    protected_swing: continuation,
                    risk_distance: round2(cont_distance),
                    risk_percent: round3(cont_distance / entry_price * 100.0),
                    reason,
                };
            }
        }

        // Fallback to wick
        let reason = format!(
            "Protected swing {} (wick, low R:R {:.1}) @ {:.2}",
            swing_type, wick_rr, wick_stop
        );
        StopLossLevel {
            price: round2(wick_stop),
            mode: StopMode::Wick,
            protected_swing: swing,
            risk_distance: round2(wick_distance),
            risk_percent: round3(wick_distance / entry_price * 100.0),
            reason,
        }
    }

    pub fn get_trailing_stop(
        &mut self,
        direction: Direction,
        current_stop: f64,
        candles: &CandleSeries,
        pdas: Option<&[Pda]>,
    ) -> Option<StopLossLevel> {
        self.find_protected_swings(candles, pdas);

        match direction {
            Direction::Long => {
                let candidates: Vec<&ProtectedSwing> = self
                    .protected_swings
                    .iter()
                    .filter(|s| s.swing_type == SwingType::Low && s.extreme > current_stop)
                    .collect();
                let best = candidates
                    .into_iter()
                    .max_by(|a, b| a.extreme.partial_cmp(&b.extreme).unwrap())?;
                Some(StopLossLevel {
                    price: round2(best.extreme),
                    mode: StopMode::Wick,
                    protected_swing: best.clone(),
                    risk_distance: 0.0,
                    risk_percent: 0.0,
                    reason: format!("Trailing stop: new protected low @ {:.2}", best.extreme),
                })
            }
            Direction::Short => {
                let candidates: Vec<&ProtectedSwing> = self
                    .protected_swings
                    .iter()
                    .filter(|s| s.swing_type == SwingType::High && s.extreme < current_stop)
                    .collect();
                let best = candidates
                    .into_iter()
                    .min_by(|a, b| a.extreme.partial_cmp(&b.extreme).unwrap())?;
                Some(StopLossLevel {
                    price: round2(best.extreme),
                    mode: StopMode::Wick,
                    protected_swing: best.clone(),
                    risk_distance: 0.0,
                    risk_percent: 0.0,
                    reason: format!("Trailing stop: new protected high @ {:.2}", best.extreme),
                })
            }
        }
    }

    // --- Internal methods ---

    fn find_raw_swings(&self, candles: &CandleSeries) -> (Vec<RawSwing>, Vec<RawSwing>) {
        let lb = self.swing_lookback;
        let len = candles.len();
        let mut highs = Vec::new();
        let mut lows = Vec::new();

        for i in lb..(len.saturating_sub(lb)) {
            let window = candles.slice(i.saturating_sub(lb), (i + lb + 1).min(len));

            if candles[i].high >= window.highs_max() {
                highs.push(RawSwing {
                    index: i,
                    timestamp: candles[i].timestamp,
                    price: candles[i].high,
                    close: candles[i].close,
                    open: candles[i].open,
                });
            }

            if candles[i].low <= window.lows_min() {
                lows.push(RawSwing {
                    index: i,
                    timestamp: candles[i].timestamp,
                    price: candles[i].low,
                    close: candles[i].close,
                    open: candles[i].open,
                });
            }
        }

        (highs, lows)
    }

    fn validate_protected_swing(
        &self,
        candles: &CandleSeries,
        swing: &RawSwing,
        swing_type: SwingType,
        pdas: Option<&[Pda]>,
    ) -> Option<ProtectedSwing> {
        let idx = swing.index;
        if idx + 2 >= candles.len() {
            return None;
        }

        let mut sweep_confirmed = false;
        let sweep_candle_count;

        match swing_type {
            SwingType::Low => {
                let start = idx.saturating_sub(20);
                let prior_lows = candles.slice(start, idx);
                if !prior_lows.is_empty() {
                    let prev_swing_low = prior_lows.lows_min();
                    if swing.price <= prev_swing_low {
                        sweep_confirmed = true;
                    }
                }

                if !sweep_confirmed {
                    if let Some(pda_list) = pdas {
                        for pda in pda_list {
                            if pda.direction == Trend::Bullish && swing.price <= pda.high {
                                sweep_confirmed = true;
                                break;
                            }
                        }
                    }
                }
            }
            SwingType::High => {
                let start = idx.saturating_sub(20);
                let prior_highs = candles.slice(start, idx);
                if !prior_highs.is_empty() {
                    let prev_swing_high = prior_highs.highs_max();
                    if swing.price >= prev_swing_high {
                        sweep_confirmed = true;
                    }
                }

                if !sweep_confirmed {
                    if let Some(pda_list) = pdas {
                        for pda in pda_list {
                            if pda.direction == Trend::Bearish && swing.price >= pda.low {
                                sweep_confirmed = true;
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Confirmation close
        let mut close_confirmed = false;
        let after_end = (idx + 6).min(candles.len());
        let after_swing = candles.slice(idx + 1, after_end);

        match swing_type {
            SwingType::Low => {
                let sweep_start = idx.saturating_sub(2);
                let sweep_series = candles.slice(sweep_start, idx + 1);
                let sweep_series_high = sweep_series.highs_max();
                for c in after_swing.iter() {
                    if c.close > sweep_series_high {
                        close_confirmed = true;
                        break;
                    }
                }
                sweep_candle_count = 3.min(idx + 1);
            }
            SwingType::High => {
                let sweep_start = idx.saturating_sub(2);
                let sweep_series = candles.slice(sweep_start, idx + 1);
                let sweep_series_low = sweep_series.lows_min();
                for c in after_swing.iter() {
                    if c.close < sweep_series_low {
                        close_confirmed = true;
                        break;
                    }
                }
                sweep_candle_count = 3.min(idx + 1);
            }
        }

        if !(sweep_confirmed || close_confirmed) {
            return None;
        }

        let body_level = match swing_type {
            SwingType::Low => swing.close.max(swing.open),
            SwingType::High => swing.close.min(swing.open),
        };

        let mut strength = 0.3;
        if sweep_confirmed {
            strength += 0.35;
        }
        if close_confirmed {
            strength += 0.35;
        }

        Some(ProtectedSwing {
            swing_type,
            extreme: swing.price,
            body_level,
            timestamp: swing.timestamp,
            sweep_confirmed,
            close_confirmed,
            strength,
            candle_count: sweep_candle_count,
        })
    }

    fn get_nearest_protected_swing(
        &self,
        entry: f64,
        direction: Direction,
    ) -> Option<&ProtectedSwing> {
        match direction {
            Direction::Long => {
                let candidates: Vec<&ProtectedSwing> = self
                    .protected_swings
                    .iter()
                    .filter(|s| s.swing_type == SwingType::Low && s.extreme < entry)
                    .collect();
                candidates
                    .into_iter()
                    .max_by(|a, b| a.extreme.partial_cmp(&b.extreme).unwrap())
            }
            Direction::Short => {
                let candidates: Vec<&ProtectedSwing> = self
                    .protected_swings
                    .iter()
                    .filter(|s| s.swing_type == SwingType::High && s.extreme > entry)
                    .collect();
                candidates
                    .into_iter()
                    .min_by(|a, b| a.extreme.partial_cmp(&b.extreme).unwrap())
            }
        }
    }

    fn is_body_mode_safe(&self, swing: &ProtectedSwing, candles: &CandleSeries) -> bool {
        let swing_candle = candles
            .iter()
            .find(|c| c.timestamp == swing.timestamp);

        let candle = match swing_candle {
            Some(c) => c,
            None => return false,
        };

        let total_range = candle.total_range();
        if total_range == 0.0 {
            return false;
        }

        let body = candle.body();
        let body_ratio = body / total_range;

        let wick_ratio = match swing.swing_type {
            SwingType::Low => candle.lower_wick() / total_range,
            SwingType::High => candle.upper_wick() / total_range,
        };

        wick_ratio <= MAX_WICK_RATIO_FOR_BODY && body_ratio >= 0.5
    }

    fn find_continuation_swing(
        &self,
        entry: f64,
        direction: Direction,
        original: &ProtectedSwing,
    ) -> Option<ProtectedSwing> {
        match direction {
            Direction::Long => {
                let candidates: Vec<&ProtectedSwing> = self
                    .protected_swings
                    .iter()
                    .filter(|s| {
                        s.swing_type == SwingType::Low
                            && s.extreme < entry
                            && s.extreme > original.extreme
                            && s.timestamp > original.timestamp
                    })
                    .collect();
                candidates
                    .into_iter()
                    .max_by(|a, b| a.extreme.partial_cmp(&b.extreme).unwrap())
                    .cloned()
            }
            Direction::Short => {
                let candidates: Vec<&ProtectedSwing> = self
                    .protected_swings
                    .iter()
                    .filter(|s| {
                        s.swing_type == SwingType::High
                            && s.extreme > entry
                            && s.extreme < original.extreme
                            && s.timestamp > original.timestamp
                    })
                    .collect();
                candidates
                    .into_iter()
                    .min_by(|a, b| a.extreme.partial_cmp(&b.extreme).unwrap())
                    .cloned()
            }
        }
    }

    fn fallback_stop(
        &self,
        entry: f64,
        direction: Direction,
        candles: &CandleSeries,
    ) -> StopLossLevel {
        let atr = calc_atr(candles, 14);
        let (stop, swing_type) = match direction {
            Direction::Long => (entry - atr * 1.5, SwingType::Low),
            Direction::Short => (entry + atr * 1.5, SwingType::High),
        };

        StopLossLevel {
            price: round2(stop),
            mode: StopMode::Wick,
            protected_swing: ProtectedSwing {
                swing_type,
                extreme: stop,
                body_level: stop,
                timestamp: candles.last().map_or(Utc::now(), |c| c.timestamp),
                sweep_confirmed: false,
                close_confirmed: false,
                strength: 0.1,
                candle_count: 0,
            },
            risk_distance: round2((entry - stop).abs()),
            risk_percent: round3((entry - stop).abs() / entry * 100.0),
            reason: format!("FALLBACK: ATR-based stop (no protected swing found) @ {:.2}", stop),
        }
    }
}

struct RawSwing {
    index: usize,
    timestamp: DateTime<Utc>,
    price: f64,
    close: f64,
    open: f64,
}

pub fn calc_atr(candles: &CandleSeries, period: usize) -> f64 {
    if candles.len() < period {
        return candles
            .last()
            .map_or(0.0, |c| c.high - c.low);
    }

    let mut trs: Vec<f64> = Vec::with_capacity(candles.len());
    trs.push(candles[0].high - candles[0].low);

    for i in 1..candles.len() {
        let hl = candles[i].high - candles[i].low;
        let hc = (candles[i].high - candles[i - 1].close).abs();
        let lc = (candles[i].low - candles[i - 1].close).abs();
        trs.push(hl.max(hc).max(lc));
    }

    let start = trs.len().saturating_sub(period);
    let slice = &trs[start..];
    slice.iter().sum::<f64>() / slice.len() as f64
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{make_bullish_trend, make_bearish_trend, make_candles};

    #[test]
    fn wick_mode_when_good_rr() {
        // Build candles with a clear swing low, then test long entry above it
        let candles = make_bullish_trend(30, 100.0);
        let mut engine = StopLossEngine::new();
        // entry near the top, TP far above, SL at swing low => good R:R
        let entry = 380.0;
        let tp = 500.0;
        let sl = engine.get_stop_loss(entry, Direction::Long, tp, &candles, None);
        // Should find a protected swing and return a stop
        assert!(sl.price < entry, "SL should be below entry for long");
        assert!(sl.risk_distance > 0.0);
    }

    #[test]
    fn fallback_stop_when_no_swings() {
        // Very few candles => no protected swings found
        let candles = make_candles(&[
            (100.0, 102.0, 99.0, 101.0),
            (101.0, 103.0, 100.0, 102.0),
        ]);
        let mut engine = StopLossEngine::new();
        let sl = engine.get_stop_loss(102.0, Direction::Long, 120.0, &candles, None);
        assert!(sl.reason.contains("FALLBACK"));
        assert!(sl.price < 102.0);
    }

    #[test]
    fn fallback_stop_for_short() {
        let candles = make_candles(&[
            (100.0, 102.0, 99.0, 101.0),
            (101.0, 103.0, 100.0, 102.0),
        ]);
        let mut engine = StopLossEngine::new();
        let sl = engine.get_stop_loss(102.0, Direction::Short, 90.0, &candles, None);
        assert!(sl.price > 102.0, "SL should be above entry for short");
    }

    #[test]
    fn trailing_stop_only_moves_favorably_long() {
        let candles = make_bullish_trend(30, 100.0);
        let mut engine = StopLossEngine::new();
        let current_stop = 95.0; // well below the trend
        let result = engine.get_trailing_stop(Direction::Long, current_stop, &candles, None);
        if let Some(new_sl) = result {
            assert!(new_sl.price > current_stop, "Trailing stop should only move up for longs");
        }
    }

    #[test]
    fn trailing_stop_only_moves_favorably_short() {
        let candles = make_bearish_trend(30, 500.0);
        let mut engine = StopLossEngine::new();
        let current_stop = 510.0; // well above the trend
        let result = engine.get_trailing_stop(Direction::Short, current_stop, &candles, None);
        if let Some(new_sl) = result {
            assert!(new_sl.price < current_stop, "Trailing stop should only move down for shorts");
        }
    }
}
