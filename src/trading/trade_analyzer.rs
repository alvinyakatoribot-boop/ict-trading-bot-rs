use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::trading::trade_record::TradeRecord;

const DIMENSIONS: &[&str] = &[
    "scale",
    "session",
    "day_of_week",
    "cisd_status",
    "stop_mode",
    "pda_type",
    "confidence_bucket",
    "cross_scale_confluence",
    "weekly_profile",
    "tp_label",
    "scale_session",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketStats {
    pub dimension: String,
    pub value: String,
    pub total: usize,
    pub wins: usize,
    pub losses: usize,
    pub win_rate: f64,
    pub avg_pnl: f64,
    pub total_pnl: f64,
    pub payoff_ratio: f64,
    pub edge: f64,
    pub sample_sufficient: bool,
}

pub struct TradeAnalyzer {
    pub min_sample: usize,
}

impl TradeAnalyzer {
    pub fn new(min_sample: usize) -> Self {
        Self { min_sample }
    }

    pub fn analyze(
        &self,
        records: &[TradeRecord],
    ) -> HashMap<String, HashMap<String, BucketStats>> {
        let closed: Vec<&TradeRecord> = records
            .iter()
            .filter(|r| r.outcome == "win" || r.outcome == "loss")
            .collect();

        let mut results = HashMap::new();
        for &dim in DIMENSIONS {
            results.insert(
                dim.to_string(),
                self.analyze_dimension(&closed, dim),
            );
        }
        results
    }

    pub fn get_negative_edge_buckets(
        &self,
        analysis: &HashMap<String, HashMap<String, BucketStats>>,
    ) -> Vec<BucketStats> {
        let mut out: Vec<BucketStats> = analysis
            .values()
            .flat_map(|dim_stats| dim_stats.values())
            .filter(|b| b.sample_sufficient && b.edge < 0.0)
            .cloned()
            .collect();
        out.sort_by(|a, b| a.edge.partial_cmp(&b.edge).unwrap());
        out
    }

    pub fn get_strongest_buckets(
        &self,
        analysis: &HashMap<String, HashMap<String, BucketStats>>,
    ) -> Vec<BucketStats> {
        let mut out: Vec<BucketStats> = analysis
            .values()
            .flat_map(|dim_stats| dim_stats.values())
            .filter(|b| b.sample_sufficient && b.edge > 0.0)
            .cloned()
            .collect();
        out.sort_by(|a, b| b.edge.partial_cmp(&a.edge).unwrap());
        out
    }

    fn analyze_dimension(
        &self,
        records: &[&TradeRecord],
        dimension: &str,
    ) -> HashMap<String, BucketStats> {
        let mut buckets: HashMap<String, Vec<&TradeRecord>> = HashMap::new();

        for r in records {
            if let Some(key) = self.extract_key(r, dimension) {
                buckets.entry(key).or_default().push(r);
            }
        }

        let mut results = HashMap::new();
        for (value, trades) in buckets {
            results.insert(
                value.clone(),
                self.compute_stats(dimension, &value, &trades),
            );
        }
        results
    }

    fn extract_key(&self, record: &TradeRecord, dimension: &str) -> Option<String> {
        let m = &record.metadata;
        match dimension {
            "scale" => Some(m.scale.clone()),
            "session" => Some(m.session.clone()),
            "day_of_week" => Some(m.day_of_week.clone()),
            "cisd_status" => Some(if m.cisd_confirmed {
                "confirmed".to_string()
            } else {
                "unconfirmed".to_string()
            }),
            "stop_mode" => Some(if m.stop_mode.is_empty() {
                "unknown".to_string()
            } else {
                m.stop_mode.clone()
            }),
            "pda_type" => Some(if m.pda_type.is_empty() {
                "none".to_string()
            } else {
                m.pda_type.clone()
            }),
            "confidence_bucket" => Some(if m.confidence >= 0.8 {
                "high_0.8+".to_string()
            } else if m.confidence >= 0.6 {
                "mid_0.6-0.8".to_string()
            } else if m.confidence >= 0.4 {
                "low_0.4-0.6".to_string()
            } else {
                "very_low_<0.4".to_string()
            }),
            "cross_scale_confluence" => Some(m.cross_scale_confluence.to_string()),
            "weekly_profile" => Some(if m.weekly_profile.is_empty() {
                "unknown".to_string()
            } else {
                m.weekly_profile.clone()
            }),
            "tp_label" => Some(if m.tp_label.is_empty() {
                "unknown".to_string()
            } else {
                m.tp_label.clone()
            }),
            "scale_session" => Some(format!("{}_{}", m.scale, m.session)),
            _ => None,
        }
    }

    fn compute_stats(
        &self,
        dimension: &str,
        value: &str,
        trades: &[&TradeRecord],
    ) -> BucketStats {
        let total = trades.len();
        let wins = trades.iter().filter(|t| t.outcome == "win").count();
        let losses = total - wins;
        let win_rate = if total > 0 {
            wins as f64 / total as f64
        } else {
            0.0
        };

        let total_pnl: f64 = trades.iter().map(|t| t.pnl).sum();
        let avg_pnl = if total > 0 {
            total_pnl / total as f64
        } else {
            0.0
        };

        let avg_win = if wins > 0 {
            trades
                .iter()
                .filter(|t| t.outcome == "win")
                .map(|t| t.pnl)
                .sum::<f64>()
                / wins as f64
        } else {
            0.0
        };

        let avg_loss = if losses > 0 {
            (trades
                .iter()
                .filter(|t| t.outcome == "loss")
                .map(|t| t.pnl)
                .sum::<f64>()
                / losses as f64)
                .abs()
        } else {
            0.0
        };

        let payoff_ratio = if avg_loss > 0.0 {
            avg_win / avg_loss
        } else {
            0.0
        };

        let edge = if total > 0 {
            (win_rate * avg_win) - ((1.0 - win_rate) * avg_loss)
        } else {
            0.0
        };

        BucketStats {
            dimension: dimension.to_string(),
            value: value.to_string(),
            total,
            wins,
            losses,
            win_rate: round4(win_rate),
            avg_pnl: round4(avg_pnl),
            total_pnl: round4(total_pnl),
            payoff_ratio: round4(payoff_ratio),
            edge: round4(edge),
            sample_sufficient: total >= self.min_sample,
        }
    }
}

fn round4(x: f64) -> f64 {
    (x * 10000.0).round() / 10000.0
}
