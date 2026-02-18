use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::config::Config;
use crate::core::cisd::CisdDetector;
use crate::core::liquidity::LiquidityDetector;
use crate::core::pd_arrays::{Pda, PdArrayDetector};
use crate::core::sessions::SessionManager;
use crate::core::stddev_projections::StdDevProjector;
use crate::core::stop_loss::StopLossEngine;
use crate::core::structure::{DealingRange, MarketStructure};
use crate::models::{CandleSeries, Direction, PdaType, Timeframe, Trend, Zone};
use crate::strategies::signals::TradeSignal;
use crate::trading::trade_record::{AlignmentInfo, TpLevelInfo};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlignmentState {
    pub timeframe: Timeframe,
    pub trend: Trend,
    pub dealing_range: Option<DealingRange>,
    pub swing_count: usize,
    pub bos_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HftSignal {
    pub scale: String,
    pub scale_name: String,
    pub direction: Direction,
    pub entry_price: f64,
    pub stop_loss: f64,
    pub take_profit: f64,
    pub pda_engaged: Pda,
    pub cisd_confirmed: bool,
    pub confidence: f64,
    pub session: String,
    pub session_weight: f64,
    pub reason: String,
    pub cross_scale_confluence: usize,
    pub stop_mode: String,
    pub stop_reason: String,
    pub tp_label: String,
    pub tp_levels: Vec<TpLevelInfo>,
    pub alignment: Vec<AlignmentInfo>,
}

impl HftSignal {
    pub fn to_trade_signal(&self) -> TradeSignal {
        TradeSignal {
            direction: self.direction,
            entry_price: self.entry_price,
            stop_loss: self.stop_loss,
            take_profit: self.take_profit,
            pda_engaged: Some(self.pda_engaged.clone()),
            cisd_confirmed: self.cisd_confirmed,
            confidence: self.confidence,
            session: self.session.clone(),
            session_weight: self.session_weight,
            reason: self.reason.clone(),
            tp_levels: Some(self.tp_levels.clone()),
        }
    }
}

pub struct HftScale {
    pub scale_key: String,
    pub name: String,
    pub entry_tf: Timeframe,
    pub alignment_tfs: Vec<Timeframe>,
    pub structure_tf: Timeframe,
    pub confirm_tf: Timeframe,
    pub weight: f64,

    pd_detector: PdArrayDetector,
    cisd_detector: CisdDetector,
    stop_engine: StopLossEngine,
    sd_projector: StdDevProjector,
    liquidity_detector: LiquidityDetector,
    alignment_analyzers: HashMap<Timeframe, MarketStructure>,
    structure_analyzer: MarketStructure,

    pub last_alignment: Vec<AlignmentState>,
    last_structure_pdas: Vec<Pda>,
}

impl HftScale {
    pub fn new(scale_key: &str, cfg: &Config) -> Self {
        let scale_cfg = &cfg.hft_scales[scale_key];
        let alignment_analyzers = scale_cfg
            .alignment_tfs
            .iter()
            .map(|&tf| (tf, MarketStructure::new()))
            .collect();

        Self {
            scale_key: scale_key.to_string(),
            name: scale_cfg.name.clone(),
            entry_tf: scale_cfg.entry_tf,
            alignment_tfs: scale_cfg.alignment_tfs.clone(),
            structure_tf: scale_cfg.structure_tf,
            confirm_tf: scale_cfg.confirm_tf,
            weight: scale_cfg.weight,
            pd_detector: PdArrayDetector::new(),
            cisd_detector: CisdDetector::new(),
            stop_engine: StopLossEngine::new(),
            sd_projector: StdDevProjector::new(),
            liquidity_detector: LiquidityDetector::new(),
            alignment_analyzers,
            structure_analyzer: MarketStructure::new(),
            last_alignment: Vec::new(),
            last_structure_pdas: Vec::new(),
        }
    }

    pub fn evaluate(
        &mut self,
        data: &HashMap<Timeframe, CandleSeries>,
        reference_price: Option<f64>,
        session: &SessionManager,
        cfg: &Config,
    ) -> Option<HftSignal> {
        let entry_df = data.get(&self.entry_tf)?;
        let struct_df = data.get(&self.structure_tf)?;
        let confirm_df = data.get(&self.confirm_tf)?;

        if entry_df.is_empty() || struct_df.is_empty() || confirm_df.is_empty() {
            return None;
        }

        // Step 1: Alignment gate
        let aligned_direction = match self.check_alignment(data) {
            Some(d) => d,
            None => {
                tracing::trace!("[EVAL] {} blocked at alignment", self.name);
                return None;
            }
        };

        // Step 2: Structure TF PDAs + Dealing Range
        self.structure_analyzer.analyze(struct_df);
        let dr = self.structure_analyzer.get_dealing_range(Some(struct_df));
        let structure_pdas = self
            .pd_detector
            .detect_all(
                struct_df,
                self.structure_tf,
                cfg.fvg_min_gap_percent,
                cfg.ob_lookback,
                cfg.breaker_lookback,
            )
            .to_vec();
        self.last_structure_pdas = structure_pdas.clone();
        let _liquidity = self.structure_analyzer.get_liquidity_levels();

        // Step 3: Judas swing detection
        if !self.detect_judas_swing(entry_df, aligned_direction, reference_price, &dr) {
            tracing::debug!("[EVAL] {} passed alignment ({:?}) but blocked at Judas swing", self.name, aligned_direction);
            return None;
        }

        // Step 4: PDA engagement
        let engaged_pda = match self.check_pda_engagement(entry_df, &structure_pdas, aligned_direction) {
            Some(p) => p,
            None => {
                tracing::debug!("[EVAL] {} passed Judas swing but blocked at PDA engagement", self.name);
                return None;
            }
        };

        // Step 5: CISD confirmation
        let struct_breakers: Vec<&Pda> = structure_pdas
            .iter()
            .filter(|p| p.pda_type == PdaType::BRK)
            .collect();

        let mut entry_pd_detector = PdArrayDetector::new();
        let entry_pdas = entry_pd_detector.detect_all(
            entry_df,
            self.entry_tf,
            cfg.fvg_min_gap_percent,
            cfg.ob_lookback,
            cfg.breaker_lookback,
        );
        let entry_breakers: Vec<&Pda> = entry_pdas
            .iter()
            .filter(|p| p.pda_type == PdaType::BRK)
            .collect();

        let all_breakers: Vec<Pda> = struct_breakers
            .iter()
            .chain(entry_breakers.iter())
            .cloned()
            .cloned()
            .collect();

        let cisds = self.cisd_detector.check(confirm_df, &all_breakers);
        let cisd_confirmed = !cisds.is_empty();
        let base_confidence = if cisd_confirmed { 0.8 } else { 0.4 };

        // Step 6: Build signal
        Some(self.build_signal(
            entry_df,
            aligned_direction,
            engaged_pda,
            &dr,
            cisd_confirmed,
            base_confidence,
            session,
            cfg,
        ))
    }

    pub fn check_alignment(
        &mut self,
        data: &HashMap<Timeframe, CandleSeries>,
    ) -> Option<Trend> {
        self.last_alignment.clear();
        let mut directions = Vec::new();

        for &tf in &self.alignment_tfs {
            let df = data.get(&tf)?;
            if df.is_empty() {
                return None;
            }

            let analyzer = self.alignment_analyzers.get_mut(&tf)?;
            let trend = analyzer.analyze(df);
            let dr = analyzer.get_dealing_range(Some(df));

            self.last_alignment.push(AlignmentState {
                timeframe: tf,
                trend,
                dealing_range: Some(dr),
                swing_count: analyzer.swing_highs.len() + analyzer.swing_lows.len(),
                bos_count: analyzer.bos_events.len(),
            });

            if trend == Trend::Neutral {
                return None;
            }

            directions.push(trend);
        }

        // All must agree
        if directions.windows(2).all(|w| w[0] == w[1]) {
            Some(directions[0])
        } else {
            None
        }
    }

    fn detect_judas_swing(
        &self,
        entry_df: &CandleSeries,
        direction: Trend,
        ref_price: Option<f64>,
        dr: &DealingRange,
    ) -> bool {
        if entry_df.is_empty() {
            return false;
        }

        let pivot = ref_price.unwrap_or(dr.equilibrium);
        if pivot == 0.0 {
            return false;
        }

        // Use wider lookback window for better Judas swing detection
        let recent = entry_df.tail(60);
        let current = match recent.last() {
            Some(c) => c.close,
            None => return false,
        };

        match direction {
            Trend::Bullish => {
                // Classic Judas: swept below pivot and reclaimed
                let went_below = recent.any_low_below(pivot);
                let came_back = current > pivot;
                if went_below && came_back {
                    return true;
                }
                // Fallback: price is in discount zone of dealing range (below equilibrium)
                // and showing reversal — this is a valid ICT setup
                current < dr.equilibrium && current > dr.low
            }
            Trend::Bearish => {
                let went_above = recent.any_high_above(pivot);
                let came_back = current < pivot;
                if went_above && came_back {
                    return true;
                }
                // Fallback: price is in premium zone (above equilibrium) and showing reversal
                current > dr.equilibrium && current < dr.high
            }
            Trend::Neutral => false,
        }
    }

    fn check_pda_engagement(
        &self,
        entry_df: &CandleSeries,
        structure_pdas: &[Pda],
        direction: Trend,
    ) -> Option<Pda> {
        if structure_pdas.is_empty() || entry_df.is_empty() {
            return None;
        }

        let recent = entry_df.tail(10);
        let recent_low = recent.lows_min();
        let recent_high = recent.highs_max();

        // First try strict zone matching (discount for bullish, premium for bearish)
        let strict_candidates: Vec<&Pda> = match direction {
            Trend::Bullish => structure_pdas
                .iter()
                .filter(|p| p.direction == Trend::Bullish && p.zone == Zone::Discount)
                .collect(),
            Trend::Bearish => structure_pdas
                .iter()
                .filter(|p| p.direction == Trend::Bearish && p.zone == Zone::Premium)
                .collect(),
            Trend::Neutral => structure_pdas
                .iter()
                .filter(|p| p.strength > 0.4)
                .collect(),
        };
        // Fallback: accept PDAs matching direction in any zone
        let candidates: Vec<&Pda> = if strict_candidates.is_empty() {
            match direction {
                Trend::Bullish => structure_pdas
                    .iter()
                    .filter(|p| p.direction == Trend::Bullish)
                    .collect(),
                Trend::Bearish => structure_pdas
                    .iter()
                    .filter(|p| p.direction == Trend::Bearish)
                    .collect(),
                Trend::Neutral => structure_pdas
                    .iter()
                    .filter(|p| p.strength > 0.3)
                    .collect(),
            }
        } else {
            strict_candidates
        };

        let mut sorted = candidates;
        sorted.sort_by(|a, b| b.strength.partial_cmp(&a.strength).unwrap());

        for pda in sorted {
            if recent_low <= pda.high && recent_high >= pda.low {
                return Some(pda.clone());
            }
        }

        None
    }

    fn build_signal(
        &mut self,
        entry_df: &CandleSeries,
        direction: Trend,
        pda: Pda,
        dr: &DealingRange,
        cisd: bool,
        confidence: f64,
        session: &SessionManager,
        _cfg: &Config,
    ) -> HftSignal {
        let current = entry_df.last().unwrap().close;
        let trade_dir = match direction {
            Trend::Bullish => Direction::Long,
            _ => Direction::Short,
        };

        // SD Projection TP
        let sd_proj = self.sd_projector.project(
            entry_df,
            direction,
            Some(&self.last_structure_pdas),
            None,
            None,
        );
        let mut take_profit = sd_proj.recommended_tp;
        let mut tp_label = sd_proj.recommended_label.clone();

        if trade_dir == Direction::Long && take_profit <= current {
            take_profit = dr.high;
            tp_label = "DR High (SD fallback)".to_string();
        } else if trade_dir == Direction::Short && take_profit >= current {
            take_profit = dr.low;
            tp_label = "DR Low (SD fallback)".to_string();
        }

        // ERL liquidity pool targeting — check both entry and structure TF pools
        let mut pools = self.liquidity_detector.detect_pools(entry_df);
        // Use structure_analyzer's swing data for HTF liquidity pools
        let htf_liq = self.structure_analyzer.get_liquidity_levels();
        // Add HTF swing highs as BSL pools and swing lows as SSL pools
        for bsl_price in &htf_liq.bsl {
            // Only add if not already covered by entry TF pools
            let covered = pools.iter().any(|p| {
                matches!(p.pool_type, crate::core::liquidity::LiquidityType::BSL)
                    && (p.price - bsl_price).abs() / bsl_price < 0.001
            });
            if !covered {
                pools.push(crate::core::liquidity::LiquidityPool {
                    pool_type: crate::core::liquidity::LiquidityType::BSL,
                    price: *bsl_price,
                    touches: 1,
                    first_touch: chrono::Utc::now(),
                    last_touch: chrono::Utc::now(),
                    swept: false,
                    strength: 0.5, // HTF single swing = moderate strength
                });
            }
        }
        for ssl_price in &htf_liq.ssl {
            let covered = pools.iter().any(|p| {
                matches!(p.pool_type, crate::core::liquidity::LiquidityType::SSL)
                    && (p.price - ssl_price).abs() / ssl_price < 0.001
            });
            if !covered {
                pools.push(crate::core::liquidity::LiquidityPool {
                    pool_type: crate::core::liquidity::LiquidityType::SSL,
                    price: *ssl_price,
                    touches: 1,
                    first_touch: chrono::Utc::now(),
                    last_touch: chrono::Utc::now(),
                    swept: false,
                    strength: 0.5,
                });
            }
        }
        if let Some(erl) = self.liquidity_detector.nearest_erl_target(&pools, current, trade_dir) {
            let erl_dist = (erl.price - current).abs();
            let sd_dist = (take_profit - current).abs();

            // Use ERL if it's a strong pool (2+ touches) and provides better R:R
            if erl.touches >= 2 {
                // If ERL is farther than SD TP, use it (more upside)
                if erl_dist > sd_dist {
                    take_profit = erl.price;
                    tp_label = format!("ERL {}x touches ({:.0})", erl.touches, erl.price);
                }
                // If ERL is closer but still meaningful (>60% of SD dist), prefer it
                // as a more reliable target (liquidity actually rests there)
                else if erl_dist > sd_dist * 0.6 && erl.strength > 0.5 {
                    take_profit = erl.price;
                    tp_label = format!("ERL {}x reliable ({:.0})", erl.touches, erl.price);
                }
            }
        }

        let mut tp_levels: Vec<TpLevelInfo> = sd_proj
            .levels
            .iter()
            .map(|l| TpLevelInfo {
                label: l.label.clone(),
                price: l.price,
                pda_confluence: l.has_pda_confluence,
                level: Some(l.level),
            })
            .collect();

        // If ERL target was used, replace TP4 (-4.5 SD) with the ERL price
        // so partial exits actually reach the liquidity pool
        if let Some(erl) = self.liquidity_detector.nearest_erl_target(&pools, current, trade_dir) {
            if erl.touches >= 2 {
                let erl_dist = (erl.price - current).abs();
                // Replace the last TP level if ERL is farther
                if let Some(tp4) = tp_levels.iter_mut().find(|l| l.level == Some(-4.5)) {
                    let tp4_dist = (tp4.price - current).abs();
                    if erl_dist > tp4_dist {
                        tp4.price = round2(erl.price);
                        tp4.label = format!("ERL {}x ({:.0})", erl.touches, erl.price);
                    }
                }
            }
        }

        // Protected swing SL
        self.stop_engine
            .find_protected_swings(entry_df, Some(&self.last_structure_pdas));
        let sl_level = self.stop_engine.get_stop_loss(
            current,
            trade_dir,
            take_profit,
            entry_df,
            Some(&self.last_structure_pdas),
        );

        // Confidence
        let mut adjusted = confidence * self.weight * session.session_weight;

        // Silver Bullet boost (10-11 AM ET)
        adjusted *= session.silver_bullet_multiplier();

        let recent = entry_df.tail(30);
        let range_pct = (recent.highs_max() - recent.lows_min()) / current;
        if range_pct > 0.03 && !cisd {
            adjusted *= 0.5;
        }

        let alignment_info: Vec<AlignmentInfo> = self
            .last_alignment
            .iter()
            .map(|a| AlignmentInfo {
                tf: a.timeframe.to_string(),
                trend: a.trend.to_string(),
                bos: a.bos_count,
            })
            .collect();

        let alignment_tfs_str: Vec<String> =
            self.alignment_tfs.iter().map(|tf| tf.to_string()).collect();

        let reason = format!(
            "[{}] {} | Aligned: {} -> {} | PDA: {}({}) @ {:.2} | CISD: {} | SL: {} ({:.2}%) | TP: {} | SD: {:.2}",
            self.name,
            trade_dir.to_string().to_uppercase(),
            alignment_tfs_str.join("+"),
            direction,
            pda.pda_type,
            pda.direction,
            pda.midpoint,
            if cisd { "YES" } else { "NO" },
            sl_level.mode,
            sl_level.risk_percent,
            tp_label,
            sd_proj.range_size,
        );

        HftSignal {
            scale: self.scale_key.clone(),
            scale_name: self.name.clone(),
            direction: trade_dir,
            entry_price: round2(current),
            stop_loss: round2(sl_level.price),
            take_profit: round2(take_profit),
            pda_engaged: pda,
            cisd_confirmed: cisd,
            confidence: round3(adjusted.min(1.0)),
            session: session.current_session.clone(),
            session_weight: session.session_weight,
            reason,
            cross_scale_confluence: 1,
            stop_mode: sl_level.mode.to_string(),
            stop_reason: sl_level.reason,
            tp_label,
            tp_levels,
            alignment: alignment_info,
        }
    }
}

pub struct FractalEngine {
    pub scales: HashMap<String, HftScale>,
}

impl FractalEngine {
    pub fn new(cfg: &Config) -> Self {
        let scales = cfg
            .hft_scales
            .keys()
            .map(|key| (key.clone(), HftScale::new(key, cfg)))
            .collect();
        Self { scales }
    }

    pub fn evaluate_all(
        &mut self,
        data: &HashMap<Timeframe, CandleSeries>,
        reference_price: Option<f64>,
        session: &SessionManager,
        cfg: &Config,
    ) -> Vec<HftSignal> {
        let mut raw_signals: Vec<HftSignal> = Vec::new();

        for (_key, scale) in &mut self.scales {
            if let Some(signal) = scale.evaluate(data, reference_price, session, cfg) {
                raw_signals.push(signal);
            }
        }

        // Cross-scale confluence
        if raw_signals.len() > 1 {
            let directions: Vec<Direction> = raw_signals.iter().map(|s| s.direction).collect();
            let total = raw_signals.len();
            for signal in &mut raw_signals {
                let agreeing = directions.iter().filter(|&&d| d == signal.direction).count();
                signal.cross_scale_confluence = agreeing;
                let bonus = (agreeing as f64 - 1.0) * cfg.cross_scale_confluence_bonus;
                signal.confidence = round3((signal.confidence + bonus).min(1.0));
                if agreeing > 1 {
                    signal.reason.push_str(&format!(
                        " | CROSS-SCALE: {}/{} entry scales agree",
                        agreeing, total
                    ));
                }
            }
        }

        // Filter by min confidence
        raw_signals.retain(|s| {
            cfg.hft_scales
                .get(&s.scale)
                .map_or(false, |sc| s.confidence >= sc.min_confidence)
        });

        raw_signals.sort_by(|a, b| b.confidence.partial_cmp(&a.confidence).unwrap());
        raw_signals
    }

    pub fn get_alignment_summary(
        &mut self,
        data: &HashMap<Timeframe, CandleSeries>,
        cfg: &Config,
    ) -> HashMap<String, AlignmentSummary> {
        let mut summary = HashMap::new();
        for (key, scale) in &mut self.scales {
            let aligned_dir = scale.check_alignment(data);
            let scale_cfg = &cfg.hft_scales[key];
            summary.insert(
                key.clone(),
                AlignmentSummary {
                    name: scale.name.clone(),
                    aligned: aligned_dir.is_some(),
                    direction: aligned_dir
                        .map_or("no alignment".to_string(), |d| d.to_string()),
                    alignment_tfs: scale_cfg
                        .alignment_tfs
                        .iter()
                        .map(|tf| tf.to_string())
                        .collect(),
                    details: scale
                        .last_alignment
                        .iter()
                        .map(|a| AlignmentDetail {
                            tf: a.timeframe.to_string(),
                            trend: a.trend.to_string(),
                        })
                        .collect(),
                },
            );
        }
        summary
    }
}

#[derive(Debug, Clone)]
pub struct AlignmentSummary {
    pub name: String,
    pub aligned: bool,
    pub direction: String,
    pub alignment_tfs: Vec<String>,
    pub details: Vec<AlignmentDetail>,
}

#[derive(Debug, Clone)]
pub struct AlignmentDetail {
    pub tf: String,
    pub trend: String,
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn round3(x: f64) -> f64 {
    (x * 1000.0).round() / 1000.0
}
