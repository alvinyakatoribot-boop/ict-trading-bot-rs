use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::{CandleSeries, Direction};

/// Tolerance for detecting "equal" highs/lows as a fraction of price
const EQUAL_LEVEL_TOLERANCE: f64 = 0.0005; // 0.05% — tight for BTC
/// Minimum number of touches to qualify as a liquidity pool
const MIN_TOUCHES: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LiquidityType {
    /// Buy-Side Liquidity — stops above swing highs / equal highs
    BSL,
    /// Sell-Side Liquidity — stops below swing lows / equal lows
    SSL,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiquidityPool {
    pub pool_type: LiquidityType,
    pub price: f64,
    pub touches: usize,
    pub first_touch: DateTime<Utc>,
    pub last_touch: DateTime<Utc>,
    pub swept: bool,
    pub strength: f64,
}

pub struct LiquidityDetector {
    swing_lookback: usize,
}

impl LiquidityDetector {
    pub fn new() -> Self {
        Self { swing_lookback: 5 }
    }

    /// Detect all liquidity pools (BSL and SSL) from candle data
    pub fn detect_pools(&self, candles: &CandleSeries) -> Vec<LiquidityPool> {
        if candles.len() < self.swing_lookback * 2 + 1 {
            return Vec::new();
        }

        let mut pools = Vec::new();

        // Find swing highs and lows
        let swing_highs = self.find_swing_highs(candles);
        let swing_lows = self.find_swing_lows(candles);

        // Group into equal highs/lows (liquidity pools)
        pools.extend(self.cluster_levels(&swing_highs, LiquidityType::BSL, candles));
        pools.extend(self.cluster_levels(&swing_lows, LiquidityType::SSL, candles));

        // Add individual unswept swing points as single-touch pools
        for (price, ts) in &swing_highs {
            let already_clustered = pools.iter().any(|p| {
                matches!(p.pool_type, LiquidityType::BSL)
                    && (p.price - price).abs() / price < EQUAL_LEVEL_TOLERANCE * 2.0
            });
            if !already_clustered {
                let swept = self.is_swept_high(*price, *ts, candles);
                pools.push(LiquidityPool {
                    pool_type: LiquidityType::BSL,
                    price: *price,
                    touches: 1,
                    first_touch: *ts,
                    last_touch: *ts,
                    swept,
                    strength: 0.3,
                });
            }
        }
        for (price, ts) in &swing_lows {
            let already_clustered = pools.iter().any(|p| {
                matches!(p.pool_type, LiquidityType::SSL)
                    && (p.price - price).abs() / price < EQUAL_LEVEL_TOLERANCE * 2.0
            });
            if !already_clustered {
                let swept = self.is_swept_low(*price, *ts, candles);
                pools.push(LiquidityPool {
                    pool_type: LiquidityType::SSL,
                    price: *price,
                    touches: 1,
                    first_touch: *ts,
                    last_touch: *ts,
                    swept,
                    strength: 0.3,
                });
            }
        }

        // Sort by strength descending
        pools.sort_by(|a, b| b.strength.partial_cmp(&a.strength).unwrap());
        pools
    }

    /// Find the nearest unswept ERL target in the trade direction
    pub fn nearest_erl_target<'a>(
        &self,
        pools: &'a [LiquidityPool],
        current_price: f64,
        direction: Direction,
    ) -> Option<&'a LiquidityPool> {
        let candidates: Vec<&LiquidityPool> = pools
            .iter()
            .filter(|p| !p.swept)
            .filter(|p| match direction {
                // For longs, target BSL (buy-side liquidity above)
                Direction::Long => {
                    matches!(p.pool_type, LiquidityType::BSL) && p.price > current_price
                }
                // For shorts, target SSL (sell-side liquidity below)
                Direction::Short => {
                    matches!(p.pool_type, LiquidityType::SSL) && p.price < current_price
                }
            })
            .collect();

        // Return the nearest one (closest to current price)
        match direction {
            Direction::Long => candidates
                .into_iter()
                .min_by(|a, b| a.price.partial_cmp(&b.price).unwrap()),
            Direction::Short => candidates
                .into_iter()
                .max_by(|a, b| a.price.partial_cmp(&b.price).unwrap()),
        }
    }

    fn find_swing_highs(&self, candles: &CandleSeries) -> Vec<(f64, DateTime<Utc>)> {
        let lb = self.swing_lookback;
        let len = candles.len();
        let mut highs = Vec::new();

        for i in lb..(len - lb) {
            let current_high = candles[i].high;
            let is_swing = (i.saturating_sub(lb)..=(i + lb).min(len - 1))
                .all(|j| j == i || candles[j].high <= current_high);
            if is_swing {
                highs.push((current_high, candles[i].timestamp));
            }
        }
        highs
    }

    fn find_swing_lows(&self, candles: &CandleSeries) -> Vec<(f64, DateTime<Utc>)> {
        let lb = self.swing_lookback;
        let len = candles.len();
        let mut lows = Vec::new();

        for i in lb..(len - lb) {
            let current_low = candles[i].low;
            let is_swing = (i.saturating_sub(lb)..=(i + lb).min(len - 1))
                .all(|j| j == i || candles[j].low >= current_low);
            if is_swing {
                lows.push((current_low, candles[i].timestamp));
            }
        }
        lows
    }

    /// Cluster nearby swing points into equal highs/lows pools
    fn cluster_levels(
        &self,
        levels: &[(f64, DateTime<Utc>)],
        pool_type: LiquidityType,
        candles: &CandleSeries,
    ) -> Vec<LiquidityPool> {
        if levels.len() < MIN_TOUCHES {
            return Vec::new();
        }

        let mut pools = Vec::new();
        let mut used = vec![false; levels.len()];

        for i in 0..levels.len() {
            if used[i] {
                continue;
            }

            let mut cluster_prices = vec![levels[i].0];
            let mut cluster_times = vec![levels[i].1];
            used[i] = true;

            for j in (i + 1)..levels.len() {
                if used[j] {
                    continue;
                }
                let avg_price = cluster_prices.iter().sum::<f64>() / cluster_prices.len() as f64;
                if (levels[j].0 - avg_price).abs() / avg_price < EQUAL_LEVEL_TOLERANCE {
                    cluster_prices.push(levels[j].0);
                    cluster_times.push(levels[j].1);
                    used[j] = true;
                }
            }

            if cluster_prices.len() >= MIN_TOUCHES {
                let avg_price = cluster_prices.iter().sum::<f64>() / cluster_prices.len() as f64;
                let first = *cluster_times.iter().min().unwrap();
                let last = *cluster_times.iter().max().unwrap();
                let touches = cluster_prices.len();

                let swept = match pool_type {
                    LiquidityType::BSL => self.is_swept_high(avg_price, last, candles),
                    LiquidityType::SSL => self.is_swept_low(avg_price, last, candles),
                };

                // Strength: more touches = stronger pool
                let strength = (0.5 + 0.15 * (touches as f64 - 1.0)).min(1.0);

                pools.push(LiquidityPool {
                    pool_type: pool_type.clone(),
                    price: round2(avg_price),
                    touches,
                    first_touch: first,
                    last_touch: last,
                    swept,
                    strength,
                });
            }
        }

        pools
    }

    fn is_swept_high(
        &self,
        level: f64,
        after: DateTime<Utc>,
        candles: &CandleSeries,
    ) -> bool {
        candles
            .iter()
            .any(|c| c.timestamp > after && c.high > level)
    }

    fn is_swept_low(
        &self,
        level: f64,
        after: DateTime<Utc>,
        candles: &CandleSeries,
    ) -> bool {
        candles
            .iter()
            .any(|c| c.timestamp > after && c.low < level)
    }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::make_candles;

    #[test]
    fn detects_equal_highs_as_bsl() {
        // Create data with two swing highs at nearly the same level
        let mut data = Vec::new();
        // Rise to 110
        for i in 0..8 {
            let v = 100.0 + i as f64 * 1.25;
            data.push((v, v + 0.5, v - 0.5, v));
        }
        // Peak at ~110
        data.push((110.0, 110.02, 109.5, 109.8));
        // Pull back
        for i in 0..8 {
            let v = 110.0 - i as f64 * 1.25;
            data.push((v, v + 0.5, v - 0.5, v));
        }
        // Rise again to ~110
        for i in 0..8 {
            let v = 100.0 + i as f64 * 1.25;
            data.push((v, v + 0.5, v - 0.5, v));
        }
        data.push((110.0, 110.03, 109.5, 109.7));
        // Pull back again
        for i in 0..8 {
            let v = 110.0 - i as f64 * 1.25;
            data.push((v, v + 0.5, v - 0.5, v));
        }

        let candles = make_candles(&data);
        let detector = LiquidityDetector::new();
        let pools = detector.detect_pools(&candles);

        let bsl: Vec<_> = pools
            .iter()
            .filter(|p| matches!(p.pool_type, LiquidityType::BSL) && p.touches >= 2)
            .collect();
        assert!(
            !bsl.is_empty(),
            "Should detect BSL from equal highs. Pools: {:?}",
            pools.iter().map(|p| (format!("{:?}", p.pool_type), p.price, p.touches)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn nearest_erl_finds_closest_target() {
        let pools = vec![
            LiquidityPool {
                pool_type: LiquidityType::BSL,
                price: 105.0,
                touches: 2,
                first_touch: Utc::now(),
                last_touch: Utc::now(),
                swept: false,
                strength: 0.65,
            },
            LiquidityPool {
                pool_type: LiquidityType::BSL,
                price: 115.0,
                touches: 3,
                first_touch: Utc::now(),
                last_touch: Utc::now(),
                swept: false,
                strength: 0.8,
            },
            LiquidityPool {
                pool_type: LiquidityType::SSL,
                price: 90.0,
                touches: 2,
                first_touch: Utc::now(),
                last_touch: Utc::now(),
                swept: false,
                strength: 0.65,
            },
        ];

        let detector = LiquidityDetector::new();

        // Long at 100 should target nearest BSL at 105
        let target = detector.nearest_erl_target(&pools, 100.0, Direction::Long);
        assert!(target.is_some());
        assert!((target.unwrap().price - 105.0).abs() < 0.01);

        // Short at 100 should target nearest SSL at 90
        let target = detector.nearest_erl_target(&pools, 100.0, Direction::Short);
        assert!(target.is_some());
        assert!((target.unwrap().price - 90.0).abs() < 0.01);
    }

    #[test]
    fn swept_pools_excluded_from_targets() {
        let pools = vec![LiquidityPool {
            pool_type: LiquidityType::BSL,
            price: 105.0,
            touches: 2,
            first_touch: Utc::now(),
            last_touch: Utc::now(),
            swept: true, // already swept
            strength: 0.65,
        }];

        let detector = LiquidityDetector::new();
        let target = detector.nearest_erl_target(&pools, 100.0, Direction::Long);
        assert!(target.is_none());
    }
}
