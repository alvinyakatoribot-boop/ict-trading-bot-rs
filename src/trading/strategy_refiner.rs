use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;

use crate::config::Config;
use crate::trading::trade_analyzer::{BucketStats, TradeAnalyzer};
use crate::trading::trade_record::TradeRecord;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Adjustment {
    pub parameter: String,
    pub old_value: f64,
    pub new_value: f64,
    pub reason: String,
    pub edge: f64,
    pub sample_size: usize,
    #[serde(default)]
    pub timestamp: String,
}

impl Adjustment {
    pub fn new(
        parameter: String,
        old_value: f64,
        new_value: f64,
        reason: String,
        edge: f64,
        sample_size: usize,
    ) -> Self {
        Self {
            parameter,
            old_value,
            new_value,
            reason,
            edge,
            sample_size,
            timestamp: Utc::now().to_rfc3339(),
        }
    }
}

// Hard floor/ceiling for each adjustable parameter
const MIN_CONFIDENCE_FLOOR: f64 = 0.3;
const MIN_CONFIDENCE_CEILING: f64 = 0.8;
const SESSION_WEIGHT_FLOOR: f64 = 0.1;
const SESSION_WEIGHT_CEILING: f64 = 2.0;

pub struct StrategyRefiner {
    pub adjustment_step: f64,
    pub min_sample: usize,
    pub analyzer: TradeAnalyzer,
    pub adjustment_history: Vec<Adjustment>,
    pub skip_combos: HashSet<String>,
    refinements_file: String,
}

impl StrategyRefiner {
    pub fn new(cfg: &Config) -> Self {
        let mut refiner = Self {
            adjustment_step: cfg.adjustment_step,
            min_sample: cfg.min_sample_per_bucket,
            analyzer: TradeAnalyzer::new(cfg.min_sample_per_bucket),
            adjustment_history: Vec::new(),
            skip_combos: HashSet::new(),
            refinements_file: format!("{}/refinements.json", cfg.log_dir),
        };
        refiner.load_state();
        refiner
    }

    pub fn refine(
        &mut self,
        records: &[TradeRecord],
        cfg: &mut Config,
    ) -> Vec<Adjustment> {
        let analysis = self.analyzer.analyze(records);
        let mut adjustments = Vec::new();

        adjustments.extend(self.adjust_min_confidence(&analysis, cfg));
        adjustments.extend(self.adjust_session_weights(&analysis, cfg));
        self.update_skip_list(&analysis);
        adjustments.extend(self.flag_stop_modes(&analysis));

        if !adjustments.is_empty() {
            self.adjustment_history.extend(adjustments.clone());
            self.save_state();
        }

        adjustments
    }

    pub fn should_skip(&self, scale: &str, session: &str) -> bool {
        self.skip_combos.contains(&format!("{}_{}", scale, session))
    }

    pub fn reset(&mut self) {
        self.adjustment_history.clear();
        self.skip_combos.clear();
        let _ = fs::remove_file(&self.refinements_file);
    }

    fn adjust_min_confidence(
        &self,
        analysis: &std::collections::HashMap<String, std::collections::HashMap<String, BucketStats>>,
        cfg: &mut Config,
    ) -> Vec<Adjustment> {
        let mut adjustments = Vec::new();
        let scale_stats = match analysis.get("scale") {
            Some(s) => s,
            None => return adjustments,
        };

        for (scale_key, bucket) in scale_stats {
            if !bucket.sample_sufficient {
                continue;
            }
            let scale_cfg = match cfg.hft_scales.get_mut(scale_key) {
                Some(c) => c,
                None => continue,
            };

            let current = scale_cfg.min_confidence;

            let new_val = if bucket.edge < 0.0 {
                (current + self.adjustment_step).min(MIN_CONFIDENCE_CEILING)
            } else if bucket.edge > 0.05 {
                (current - self.adjustment_step).max(MIN_CONFIDENCE_FLOOR)
            } else {
                continue;
            };

            if (new_val - current).abs() > f64::EPSILON {
                let new_val = round4(new_val);
                scale_cfg.min_confidence = new_val;
                adjustments.push(Adjustment::new(
                    format!("HFT_SCALES.{}.min_confidence", scale_key),
                    current,
                    new_val,
                    format!("scale {} edge={:+.4}", scale_key, bucket.edge),
                    bucket.edge,
                    bucket.total,
                ));
            }
        }

        adjustments
    }

    fn adjust_session_weights(
        &self,
        analysis: &std::collections::HashMap<String, std::collections::HashMap<String, BucketStats>>,
        cfg: &mut Config,
    ) -> Vec<Adjustment> {
        let mut adjustments = Vec::new();
        let session_stats = match analysis.get("session") {
            Some(s) => s,
            None => return adjustments,
        };

        for (session_key, bucket) in session_stats {
            if !bucket.sample_sufficient {
                continue;
            }
            let current = match cfg.session_weights.get(session_key) {
                Some(&v) => v,
                None => continue,
            };

            let new_val = if bucket.edge < 0.0 {
                (current - self.adjustment_step).max(SESSION_WEIGHT_FLOOR)
            } else if bucket.edge > 0.05 {
                (current + self.adjustment_step).min(SESSION_WEIGHT_CEILING)
            } else {
                continue;
            };

            if (new_val - current).abs() > f64::EPSILON {
                let new_val = round4(new_val);
                cfg.session_weights
                    .insert(session_key.clone(), new_val);
                adjustments.push(Adjustment::new(
                    format!("SESSION_WEIGHTS.{}", session_key),
                    current,
                    new_val,
                    format!("session {} edge={:+.4}", session_key, bucket.edge),
                    bucket.edge,
                    bucket.total,
                ));
            }
        }

        adjustments
    }

    fn update_skip_list(
        &mut self,
        analysis: &std::collections::HashMap<String, std::collections::HashMap<String, BucketStats>>,
    ) {
        let combo_stats = match analysis.get("scale_session") {
            Some(s) => s,
            None => return,
        };

        for (combo_key, bucket) in combo_stats {
            if bucket.total >= 20 && bucket.edge < -0.15 {
                self.skip_combos.insert(combo_key.clone());
            } else if self.skip_combos.contains(combo_key) && bucket.edge >= 0.0 {
                self.skip_combos.remove(combo_key);
            }
        }
    }

    fn flag_stop_modes(
        &self,
        analysis: &std::collections::HashMap<String, std::collections::HashMap<String, BucketStats>>,
    ) -> Vec<Adjustment> {
        let mut adjustments = Vec::new();
        let stop_stats = match analysis.get("stop_mode") {
            Some(s) => s,
            None => return adjustments,
        };

        for (mode, bucket) in stop_stats {
            if bucket.sample_sufficient && bucket.edge < -0.1 {
                adjustments.push(Adjustment::new(
                    format!("WARNING:stop_mode.{}", mode),
                    0.0,
                    0.0,
                    format!(
                        "stop mode '{}' has negative edge={:+.4} (n={}, wr={:.1}%)",
                        mode,
                        bucket.edge,
                        bucket.total,
                        bucket.win_rate * 100.0
                    ),
                    bucket.edge,
                    bucket.total,
                ));
            }
        }

        adjustments
    }

    fn save_state(&self) {
        let state = serde_json::json!({
            "adjustment_history": self.adjustment_history,
            "skip_combos": self.skip_combos.iter().collect::<Vec<_>>(),
        });

        if let Some(parent) = std::path::Path::new(&self.refinements_file).parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&state) {
            let _ = fs::write(&self.refinements_file, json);
        }
    }

    fn load_state(&mut self) {
        if let Ok(content) = fs::read_to_string(&self.refinements_file) {
            if let Ok(state) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Ok(history) = serde_json::from_value::<Vec<Adjustment>>(
                    state["adjustment_history"].clone(),
                ) {
                    self.adjustment_history = history;
                }
                if let Some(combos) = state["skip_combos"].as_array() {
                    self.skip_combos = combos
                        .iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect();
                }
            }
        }
    }
}

fn round4(x: f64) -> f64 {
    (x * 10000.0).round() / 10000.0
}
