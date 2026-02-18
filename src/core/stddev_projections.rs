use serde::{Deserialize, Serialize};

use crate::core::pd_arrays::Pda;
use crate::models::{CandleSeries, Trend};

const DEVIATION_LEVELS: &[f64] = &[-1.0, -2.0, -4.0, -4.5];
const PDA_CONFLUENCE_TOLERANCE: f64 = 0.15;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviationLevel {
    pub level: f64,
    pub price: f64,
    pub label: String,
    pub has_pda_confluence: bool,
    #[serde(skip)]
    pub confluence_pda: Option<Pda>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdProjection {
    pub direction: Trend,
    pub anchor_high: f64,
    pub anchor_low: f64,
    pub range_size: f64,
    pub levels: Vec<DeviationLevel>,
    pub recommended_tp: f64,
    pub recommended_label: String,
}

impl SdProjection {
    fn empty(direction: Trend) -> Self {
        Self {
            direction,
            anchor_high: 0.0,
            anchor_low: 0.0,
            range_size: 0.0,
            levels: Vec::new(),
            recommended_tp: 0.0,
            recommended_label: String::new(),
        }
    }
}

pub struct StdDevProjector {
    pub projections: Vec<SdProjection>,
}

impl StdDevProjector {
    pub fn new() -> Self {
        Self {
            projections: Vec::new(),
        }
    }

    pub fn project(
        &mut self,
        candles: &CandleSeries,
        direction: Trend,
        pdas: Option<&[Pda]>,
        anchor_high: Option<f64>,
        anchor_low: Option<f64>,
    ) -> SdProjection {
        let (manip_high, manip_low) = match (anchor_high, anchor_low) {
            (Some(h), Some(l)) => (h, l),
            _ => self.find_manipulation_leg(candles, direction),
        };

        if manip_high <= manip_low {
            return SdProjection::empty(direction);
        }

        let range_size = manip_high - manip_low;

        let mut levels: Vec<DeviationLevel> = DEVIATION_LEVELS
            .iter()
            .map(|&dev| {
                let price = match direction {
                    Trend::Bullish => manip_high + dev.abs() * range_size,
                    Trend::Bearish => manip_low - dev.abs() * range_size,
                    Trend::Neutral => manip_high, // shouldn't happen
                };

                let label = match dev {
                    x if x == -1.0 => "TP1 (-1 SD) 50%".to_string(),
                    x if x == -2.0 => "TP2 (-2 SD) 16.7%".to_string(),
                    x if x == -4.0 => "TP3 (-4 SD) 16.7%".to_string(),
                    x if x == -4.5 => "TP4 (-4.5 SD) 16.7%".to_string(),
                    _ => format!("SD {}", dev),
                };

                DeviationLevel {
                    level: dev,
                    price: round2(price),
                    label,
                    has_pda_confluence: false,
                    confluence_pda: None,
                }
            })
            .collect();

        // Check PDA confluence
        if let Some(pda_list) = pdas {
            let tolerance = range_size * PDA_CONFLUENCE_TOLERANCE;
            for lvl in &mut levels {
                for pda in pda_list {
                    if (pda.midpoint - lvl.price).abs() <= tolerance {
                        lvl.has_pda_confluence = true;
                        lvl.confluence_pda = Some(pda.clone());
                        break;
                    }
                }
            }
        }

        let recommended = self.pick_recommended_tp(&levels, direction, candles);

        let projection = SdProjection {
            direction,
            anchor_high: manip_high,
            anchor_low: manip_low,
            range_size: round2(range_size),
            recommended_tp: recommended.price,
            recommended_label: recommended.label.clone(),
            levels,
        };

        self.projections.push(projection.clone());
        projection
    }

    pub fn find_confluence_zones(&self, projections: &[SdProjection]) -> Vec<ConfluenceZone> {
        let mut all_levels: Vec<LevelInfo> = Vec::new();
        for proj in projections {
            for lvl in &proj.levels {
                all_levels.push(LevelInfo {
                    price: lvl.price,
                    level: lvl.level,
                    range_size: proj.range_size,
                });
            }
        }

        if all_levels.len() < 2 {
            return Vec::new();
        }

        all_levels.sort_by(|a, b| a.price.partial_cmp(&b.price).unwrap());

        let mut zones = Vec::new();
        for i in 0..all_levels.len() - 1 {
            let a = &all_levels[i];
            let b = &all_levels[i + 1];
            let avg_range = (a.range_size + b.range_size) / 2.0;
            if (a.price - b.price).abs() <= avg_range * PDA_CONFLUENCE_TOLERANCE {
                let strength = if a.level == -4.0 || b.level == -4.0 {
                    "high"
                } else {
                    "moderate"
                };
                zones.push(ConfluenceZone {
                    price: round2((a.price + b.price) / 2.0),
                    levels: vec![a.level, b.level],
                    strength: strength.to_string(),
                });
            }
        }

        zones
    }

    fn find_manipulation_leg(&self, candles: &CandleSeries, direction: Trend) -> (f64, f64) {
        if candles.len() < 10 {
            return (candles.highs_max(), candles.lows_min());
        }

        // Use larger lookback for better manipulation leg detection
        // ~2-4 hours worth of candles regardless of timeframe
        let lookback = 80.min(candles.len());
        let recent = candles.tail(lookback);

        match direction {
            Trend::Bullish => {
                let (manip_low, manip_high) = self.find_bullish_manip_leg(&recent);
                (manip_high, manip_low)
            }
            Trend::Bearish => {
                let (manip_high, manip_low) = self.find_bearish_manip_leg(&recent);
                (manip_high, manip_low)
            }
            Trend::Neutral => (candles.highs_max(), candles.lows_min()),
        }
    }

    fn find_bullish_manip_leg(&self, candles: &CandleSeries) -> (f64, f64) {
        let low_pos = candles
            .low_idx_min()
            .unwrap_or(0);

        let lookback_start = low_pos.saturating_sub(15);
        let pre_sweep = candles.slice(lookback_start, low_pos + 1);
        let manip_high = pre_sweep.highs_max();
        let manip_low = candles[low_pos].low;

        (manip_low, manip_high)
    }

    fn find_bearish_manip_leg(&self, candles: &CandleSeries) -> (f64, f64) {
        let high_pos = candles
            .high_idx_max()
            .unwrap_or(0);

        let lookback_start = high_pos.saturating_sub(15);
        let pre_sweep = candles.slice(lookback_start, high_pos + 1);
        let manip_low = pre_sweep.lows_min();
        let manip_high = candles[high_pos].high;

        (manip_high, manip_low)
    }

    fn pick_recommended_tp<'a>(
        &self,
        levels: &'a [DeviationLevel],
        direction: Trend,
        candles: &CandleSeries,
    ) -> &'a DeviationLevel {
        let current = candles.last().map_or(0.0, |c| c.close);

        let sd_2 = levels.iter().find(|l| l.level == -2.0);
        let sd_4 = levels.iter().find(|l| l.level == -4.0);
        let sd_45 = levels.iter().find(|l| l.level == -4.5);

        let already_past_2 = sd_2.map_or(false, |l| match direction {
            Trend::Bullish => current > l.price,
            Trend::Bearish => current < l.price,
            Trend::Neutral => false,
        });

        if already_past_2 {
            if let Some(l) = sd_45 {
                return l;
            }
        }

        if let Some(l) = sd_45 {
            return l;
        }
        if let Some(l) = sd_4 {
            return l;
        }
        if let Some(l) = sd_2 {
            return l;
        }

        &levels[0]
    }
}

#[derive(Debug, Clone)]
pub struct ConfluenceZone {
    pub price: f64,
    pub levels: Vec<f64>,
    pub strength: String,
}

struct LevelInfo {
    price: f64,
    level: f64,
    range_size: f64,
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{make_bullish_trend, make_bearish_trend};

    #[test]
    fn bullish_projection_levels() {
        let candles = make_bullish_trend(30, 100.0);
        let mut proj = StdDevProjector::new();
        let result = proj.project(
            &candles,
            Trend::Bullish,
            None,
            Some(200.0),  // anchor_high
            Some(100.0),  // anchor_low
        );
        assert_eq!(result.direction, Trend::Bullish);
        assert!((result.range_size - 100.0).abs() < 0.01);
        assert_eq!(result.levels.len(), 4);
        // TP1 = anchor_high + 1.0 * range = 200 + 100 = 300
        assert!((result.levels[0].price - 300.0).abs() < 0.01);
        // TP4 = anchor_high + 4.5 * range = 200 + 450 = 650
        assert!((result.levels[3].price - 650.0).abs() < 0.01);
    }

    #[test]
    fn bearish_projection_levels() {
        let candles = make_bearish_trend(30, 500.0);
        let mut proj = StdDevProjector::new();
        let result = proj.project(
            &candles,
            Trend::Bearish,
            None,
            Some(500.0),
            Some(400.0),
        );
        assert_eq!(result.direction, Trend::Bearish);
        // TP1 = anchor_low - 1.0 * range = 400 - 100 = 300
        assert!((result.levels[0].price - 300.0).abs() < 0.01);
    }

    #[test]
    fn pda_confluence_detection() {
        use crate::models::{PdaType, Timeframe, Zone};
        let candles = make_bullish_trend(30, 100.0);
        let pda = Pda {
            pda_type: PdaType::OB,
            direction: Trend::Bullish,
            zone: Zone::Discount,
            high: 302.0,
            low: 298.0,
            midpoint: 300.0,  // very close to TP1 at 300
            timestamp: chrono::Utc::now(),
            timeframe: Timeframe::M1,
            strength: 0.8,
        };
        let mut proj = StdDevProjector::new();
        let result = proj.project(
            &candles,
            Trend::Bullish,
            Some(&[pda]),
            Some(200.0),
            Some(100.0),
        );
        // TP1 at 300, PDA midpoint at 300 => confluence
        assert!(result.levels[0].has_pda_confluence);
    }

    #[test]
    fn empty_projection_when_anchors_invalid() {
        let candles = make_bullish_trend(30, 100.0);
        let mut proj = StdDevProjector::new();
        let result = proj.project(
            &candles,
            Trend::Bullish,
            None,
            Some(100.0),
            Some(200.0), // low > high = invalid
        );
        assert!(result.levels.is_empty());
    }
}
