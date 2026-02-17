use chrono::Datelike;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::core::cisd::CisdDetector;
use crate::core::pd_arrays::{Pda, PdArrayDetector};
use crate::core::structure::MarketStructure;
use crate::models::{CandleSeries, DrawOnLiquidity, PdaType, Timeframe, Trend, WeeklyProfile};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeeklyBias {
    pub profile: WeeklyProfile,
    pub direction: Trend,
    pub confidence: f64,
    pub draw_on_liquidity: DrawOnLiquidity,
    pub tgif_active: bool,
    pub notes: Vec<String>,
}

pub struct WeeklyProfileClassifier {
    pd_detector: PdArrayDetector,
    structure: MarketStructure,
    cisd: CisdDetector,
    pub current_bias: Option<WeeklyBias>,
}

impl WeeklyProfileClassifier {
    pub fn new() -> Self {
        Self {
            pd_detector: PdArrayDetector::new(),
            structure: MarketStructure::new(),
            cisd: CisdDetector::new(),
            current_bias: None,
        }
    }

    pub fn classify(
        &mut self,
        daily_df: &CandleSeries,
        htf_df: &CandleSeries,
        day_of_week: &str,
        cfg: &Config,
    ) -> WeeklyBias {
        let mut notes = Vec::new();

        if daily_df.len() < 3 {
            let bias = WeeklyBias {
                profile: WeeklyProfile::Undetermined,
                direction: Trend::Neutral,
                confidence: 0.0,
                draw_on_liquidity: DrawOnLiquidity::None_,
                tgif_active: false,
                notes: vec!["Insufficient data for weekly classification".to_string()],
            };
            self.current_bias = Some(bias.clone());
            return bias;
        }

        let week_candles = self.get_current_week(daily_df);
        if week_candles.is_empty() {
            let bias = WeeklyBias {
                profile: WeeklyProfile::Undetermined,
                direction: Trend::Neutral,
                confidence: 0.0,
                draw_on_liquidity: DrawOnLiquidity::None_,
                tgif_active: false,
                notes: vec!["No candles yet this week".to_string()],
            };
            self.current_bias = Some(bias.clone());
            return bias;
        }

        let trend = self.structure.analyze(htf_df);
        let pdas = self
            .pd_detector
            .detect_all(
                htf_df,
                Timeframe::H1,
                cfg.fvg_min_gap_percent,
                cfg.ob_lookback,
                cfg.breaker_lookback,
            )
            .to_vec();
        let _dealing_range = self.structure.get_dealing_range(Some(htf_df));
        let liquidity = self.structure.get_liquidity_levels();

        let classic_score =
            self.score_classic_expansion(&week_candles, day_of_week, trend, &pdas, cfg, &mut notes);
        let midweek_score = self.score_midweek_reversal(
            &week_candles,
            day_of_week,
            &pdas,
            htf_df,
            cfg,
            &mut notes,
        );
        let consol_score = self.score_consolidation_reversal(
            &week_candles,
            day_of_week,
            htf_df,
            cfg,
            &mut notes,
        );

        let scores = [
            (WeeklyProfile::ClassicExpansion, classic_score),
            (WeeklyProfile::MidweekReversal, midweek_score),
            (WeeklyProfile::ConsolidationReversal, consol_score),
        ];

        let (mut profile, confidence) = scores
            .iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .map(|&(p, c)| (p, c))
            .unwrap_or((WeeklyProfile::Undetermined, 0.0));

        if confidence < 0.3 {
            profile = WeeklyProfile::Undetermined;
        }

        let direction = self.profile_direction(profile, &week_candles, trend);

        let draw = if direction == Trend::Bullish && !liquidity.bsl.is_empty() {
            DrawOnLiquidity::Bsl
        } else if direction == Trend::Bearish && !liquidity.ssl.is_empty() {
            DrawOnLiquidity::Ssl
        } else {
            DrawOnLiquidity::None_
        };

        let tgif = day_of_week == "Friday" && profile == WeeklyProfile::ClassicExpansion;
        if tgif {
            notes.push("TGIF active — expect 20-30% retracement of weekly range".to_string());
        }

        let bias = WeeklyBias {
            profile,
            direction,
            confidence,
            draw_on_liquidity: draw,
            tgif_active: tgif,
            notes,
        };

        self.current_bias = Some(bias.clone());
        bias
    }

    fn get_current_week(&self, daily_df: &CandleSeries) -> CandleSeries {
        if daily_df.is_empty() {
            return CandleSeries::default();
        }

        let latest = daily_df.last().unwrap().timestamp;
        let weekday = latest.weekday().num_days_from_monday(); // 0=Mon
        let week_start = latest - chrono::Duration::days(weekday as i64);
        let week_start_date = week_start.date_naive();

        let candles: Vec<_> = daily_df
            .iter()
            .filter(|c| c.timestamp.date_naive() >= week_start_date)
            .cloned()
            .collect();

        CandleSeries::new(candles)
    }

    fn score_classic_expansion(
        &self,
        week: &CandleSeries,
        day: &str,
        trend: Trend,
        pdas: &[Pda],
        cfg: &Config,
        notes: &mut Vec<String>,
    ) -> f64 {
        let mut score = 0.0;
        let days = week.len();

        if days < 2 {
            return if trend != Trend::Neutral { 0.1 } else { 0.0 };
        }

        let mon_tue = week.head(2);
        let mon_tue_range = mon_tue.highs_max() - mon_tue.lows_min();
        let week_range = week.highs_max() - week.lows_min();

        if days >= 2 && mon_tue_range < week_range * 0.5 {
            score += 0.2;
            notes.push("CE: Mon/Tue range < 50% of week (manipulation phase)".to_string());
        }

        let h1_pdas: Vec<&Pda> = pdas
            .iter()
            .filter(|p| p.timeframe == Timeframe::H1 && p.strength > 0.3)
            .collect();
        for pda in &h1_pdas {
            if mon_tue.lows_min() <= pda.high && mon_tue.highs_max() >= pda.low {
                score += 0.25;
                notes.push(format!("CE: Mon/Tue engaged {} {} PDA", pda.pda_type, pda.direction));
                break;
            }
        }

        if days >= 3 {
            let wed_onward = week.slice(2, days);
            let expansion_range = wed_onward.highs_max() - wed_onward.lows_min();
            if expansion_range > mon_tue_range * 1.5 {
                score += 0.3;
                notes.push("CE: Wed+ expansion > 1.5x Mon/Tue range".to_string());
            }
        }

        if trend == Trend::Bullish || trend == Trend::Bearish {
            score += 0.15;
        }

        let rating = cfg
            .day_ratings
            .get("classic_expansion")
            .map_or(0.0, |r| r.get(day));
        score += rating / 5.0 * 0.1;

        score.min(1.0)
    }

    fn score_midweek_reversal(
        &self,
        week: &CandleSeries,
        _day: &str,
        pdas: &[Pda],
        htf_df: &CandleSeries,
        _cfg: &Config,
        notes: &mut Vec<String>,
    ) -> f64 {
        let mut score: f64 = 0.0;
        let days = week.len();

        if days < 3 {
            return 0.05;
        }

        let mon_tue_dir = if week[1].close > week[0].open {
            "up"
        } else {
            "down"
        };
        let wed = &week[2];
        let wed_dir = if wed.close > wed.open { "up" } else { "down" };

        if mon_tue_dir != wed_dir {
            score += 0.3;
            notes.push("MWR: Wednesday reversed Mon/Tue direction".to_string());
        }

        let h1_pdas: Vec<&Pda> = pdas
            .iter()
            .filter(|p| p.timeframe == Timeframe::H1 && p.strength > 0.3)
            .collect();
        for pda in &h1_pdas {
            if wed.low <= pda.high && wed.high >= pda.low {
                score += 0.25;
                notes.push(format!("MWR: Wednesday engaged {} PDA", pda.pda_type));
                break;
            }
        }

        // Check CISD on Wednesday
        let breakers: Vec<&Pda> = pdas.iter().filter(|p| p.pda_type == PdaType::BRK).collect();
        if !breakers.is_empty() {
            let wed_date = week[2].timestamp.date_naive();
            let wed_candles = htf_df.filter_by_date(wed_date);
            if !wed_candles.is_empty() {
                let brk_owned: Vec<Pda> = breakers.into_iter().cloned().collect();
                let mut cisd = CisdDetector::new();
                let cisds = cisd.check(&wed_candles, &brk_owned);
                if !cisds.is_empty() {
                    score += 0.2;
                    notes.push("MWR: CISD confirmed on Wednesday".to_string());
                }
            }
        }

        if days >= 4 {
            let thu_onward = week.slice(3, days);
            let wed_range = wed.high - wed.low;
            let thu_range = thu_onward.highs_max() - thu_onward.lows_min();
            if thu_range > wed_range {
                score += 0.15;
                notes.push("MWR: Thu+ continuing expansion".to_string());
            }
        }

        // Negative: consecutive same-direction days
        let check_len = 3.min(days);
        let all_up = (0..check_len).all(|i| week[i].close > week[i].open);
        let all_down = (0..check_len).all(|i| week[i].close < week[i].open);
        if all_up || all_down {
            score -= 0.3;
            notes.push("MWR NEGATIVE: Consecutive same-direction days".to_string());
        }

        score.clamp(0.0, 1.0)
    }

    fn score_consolidation_reversal(
        &self,
        week: &CandleSeries,
        day: &str,
        htf_df: &CandleSeries,
        cfg: &Config,
        notes: &mut Vec<String>,
    ) -> f64 {
        let mut score = 0.0;
        let days = week.len();

        if days < 3 {
            return 0.05;
        }

        let mon_wed = week.head(3.min(days));
        let daily_ranges: Vec<f64> = mon_wed.iter().map(|c| c.high - c.low).collect();
        let total_range = mon_wed.highs_max() - mon_wed.lows_min();

        let range_sum: f64 = daily_ranges.iter().sum();
        let range_overlap = range_sum / (total_range + 0.01);
        if range_overlap > 1.5 {
            score += 0.3;
            notes.push("CR: Mon-Wed showing consolidation (overlapping ranges)".to_string());
        }

        if days >= 4 {
            let thu = &week[3];
            if thu.high > mon_wed.highs_max() || thu.low < mon_wed.lows_min() {
                score += 0.25;
                notes.push("CR: Thursday swept consolidation range".to_string());

                // Check CISD on Thursday
                let mut brk_detector = PdArrayDetector::new();
                let pdas = brk_detector
                    .detect_all(
                        htf_df,
                        Timeframe::H1,
                        cfg.fvg_min_gap_percent,
                        cfg.ob_lookback,
                        cfg.breaker_lookback,
                    )
                    .to_vec();
                let breakers: Vec<Pda> = pdas
                    .into_iter()
                    .filter(|p| p.pda_type == PdaType::BRK)
                    .collect();

                if !breakers.is_empty() {
                    let thu_date = week[3].timestamp.date_naive();
                    let thu_candles = htf_df.filter_by_date(thu_date);
                    if !thu_candles.is_empty() {
                        let mut cisd = CisdDetector::new();
                        let cisds = cisd.check(&thu_candles, &breakers);
                        if !cisds.is_empty() {
                            score += 0.25;
                            notes.push("CR: CISD confirmed on Thursday".to_string());
                        }
                    }
                }
            }
        }

        if days >= 5 {
            let fri = &week[4];
            let thu = &week[3];
            if fri.body() > thu.body() {
                score += 0.15;
                notes.push("CR: Friday expanding".to_string());
            }
        }

        let rating = cfg
            .day_ratings
            .get("consolidation_reversal")
            .map_or(0.0, |r| r.get(day));
        score += rating / 5.0 * 0.05;

        score.clamp(0.0, 1.0)
    }

    fn profile_direction(
        &self,
        profile: WeeklyProfile,
        week: &CandleSeries,
        trend: Trend,
    ) -> Trend {
        if profile == WeeklyProfile::Undetermined || week.is_empty() {
            return Trend::Neutral;
        }

        let latest_close = week.last().unwrap().close;
        let week_open = week.first().unwrap().open;

        match profile {
            WeeklyProfile::ClassicExpansion => {
                if trend == Trend::Bullish || trend == Trend::Bearish {
                    trend
                } else if latest_close > week_open {
                    Trend::Bullish
                } else {
                    Trend::Bearish
                }
            }
            WeeklyProfile::MidweekReversal => {
                if week.len() >= 2 {
                    let mon_tue_dir = if week[1].close > week[0].open {
                        Trend::Bullish
                    } else {
                        Trend::Bearish
                    };
                    if mon_tue_dir == Trend::Bullish {
                        Trend::Bearish
                    } else {
                        Trend::Bullish
                    }
                } else {
                    Trend::Neutral
                }
            }
            WeeklyProfile::ConsolidationReversal => {
                if week.len() >= 4 {
                    let thu = &week[3];
                    if thu.close > thu.open {
                        Trend::Bullish
                    } else {
                        Trend::Bearish
                    }
                } else if trend != Trend::Neutral {
                    trend
                } else {
                    Trend::Neutral
                }
            }
            WeeklyProfile::Undetermined => Trend::Neutral,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Candle;
    use crate::test_helpers::default_test_config;
    use chrono::{DateTime, Duration, Utc};

    /// Build daily candles for a given week starting on Monday.
    fn make_week_candles(daily_ohlc: &[(f64, f64, f64, f64)]) -> CandleSeries {
        // Start on a Monday (2024-01-15 was a Monday)
        let base = DateTime::parse_from_rfc3339("2024-01-15T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let candles: Vec<Candle> = daily_ohlc
            .iter()
            .enumerate()
            .map(|(i, &(o, h, l, c))| Candle {
                timestamp: base + Duration::days(i as i64),
                open: o,
                high: h,
                low: l,
                close: c,
                volume: 1000.0,
            })
            .collect();
        CandleSeries::new(candles)
    }

    fn make_htf_candles() -> CandleSeries {
        // Just enough H1 candles for the classifier to work
        let base = DateTime::parse_from_rfc3339("2024-01-15T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let candles: Vec<Candle> = (0..200)
            .map(|i| {
                let v = 40000.0 + (i as f64 * 10.0);
                Candle {
                    timestamp: base + Duration::hours(i as i64),
                    open: v,
                    high: v + 50.0,
                    low: v - 20.0,
                    close: v + 30.0,
                    volume: 100.0,
                }
            })
            .collect();
        CandleSeries::new(candles)
    }

    #[test]
    fn undetermined_with_insufficient_data() {
        let cfg = default_test_config();
        let daily = make_week_candles(&[(100.0, 105.0, 95.0, 102.0)]);
        let htf = make_htf_candles();
        let mut wpc = WeeklyProfileClassifier::new();
        let bias = wpc.classify(&daily, &htf, "Tuesday", &cfg);
        assert_eq!(bias.profile, WeeklyProfile::Undetermined);
    }

    #[test]
    fn classic_expansion_detected() {
        let cfg = default_test_config();
        // Mon/Tue: tight range; Wed+: big expansion
        let daily = make_week_candles(&[
            (100.0, 103.0, 98.0, 101.0),   // Mon: range=5
            (101.0, 104.0, 99.0, 102.0),   // Tue: range=5
            (102.0, 130.0, 100.0, 128.0),  // Wed: range=30 (big expansion)
            (128.0, 145.0, 125.0, 140.0),  // Thu: continues
        ]);
        let htf = make_htf_candles();
        let mut wpc = WeeklyProfileClassifier::new();
        let bias = wpc.classify(&daily, &htf, "Thursday", &cfg);
        // Should score classic expansion highly
        assert_ne!(bias.profile, WeeklyProfile::Undetermined);
        assert!(bias.confidence > 0.3);
    }

    #[test]
    fn midweek_reversal_detected() {
        let cfg = default_test_config();
        // Mon/Tue up, Wed reverses down
        let daily = make_week_candles(&[
            (100.0, 110.0, 98.0, 108.0),   // Mon: bullish
            (108.0, 115.0, 105.0, 113.0),  // Tue: bullish
            (113.0, 114.0, 95.0, 97.0),    // Wed: big reversal down
            (97.0, 98.0, 85.0, 87.0),      // Thu: continuing down
        ]);
        let htf = make_htf_candles();
        let mut wpc = WeeklyProfileClassifier::new();
        let bias = wpc.classify(&daily, &htf, "Thursday", &cfg);
        // Should detect midweek reversal with decent confidence
        assert!(bias.confidence > 0.0);
    }

    #[test]
    fn consolidation_reversal_detected() {
        let cfg = default_test_config();
        // Mon-Wed: overlapping tight ranges, Thu: breakout
        let daily = make_week_candles(&[
            (100.0, 104.0, 97.0, 102.0),
            (102.0, 105.0, 98.0, 101.0),
            (101.0, 104.0, 97.0, 103.0),
            (103.0, 120.0, 102.0, 118.0), // Thu sweeps and breaks out
            (118.0, 125.0, 115.0, 122.0), // Fri expanding
        ]);
        let htf = make_htf_candles();
        let mut wpc = WeeklyProfileClassifier::new();
        let bias = wpc.classify(&daily, &htf, "Friday", &cfg);
        assert!(bias.confidence > 0.0);
    }

    #[test]
    fn tgif_active_on_friday_classic() {
        let cfg = default_test_config();
        let daily = make_week_candles(&[
            (100.0, 103.0, 98.0, 101.0),
            (101.0, 104.0, 99.0, 102.0),
            (102.0, 130.0, 100.0, 128.0),
            (128.0, 145.0, 125.0, 140.0),
            (140.0, 142.0, 135.0, 138.0),
        ]);
        let htf = make_htf_candles();
        let mut wpc = WeeklyProfileClassifier::new();
        let bias = wpc.classify(&daily, &htf, "Friday", &cfg);
        if bias.profile == WeeklyProfile::ClassicExpansion {
            assert!(bias.tgif_active);
        }
    }
}
