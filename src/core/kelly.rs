use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const MIN_SAMPLE_SIZE: usize = 20;
const DEFAULT_FRACTION: f64 = 0.005;
const KELLY_MULTIPLIER: f64 = 0.5;
const MAX_KELLY_FRACTION: f64 = 0.06;
const MIN_KELLY_FRACTION: f64 = 0.002;
const ROLLING_WINDOW: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KellyResult {
    pub full_kelly: f64,
    pub applied_fraction: f64,
    pub win_rate: f64,
    pub loss_rate: f64,
    pub payoff_ratio: f64,
    pub sample_size: usize,
    pub using_default: bool,
    pub edge: f64,
}

/// Trait for anything with a PnL and a reason string (for scale filtering).
pub trait HasPnl {
    fn pnl(&self) -> f64;
    fn reason(&self) -> &str;
}

pub struct KellyCriterion {
    scale_results: HashMap<String, KellyResult>,
}

impl KellyCriterion {
    pub fn new() -> Self {
        Self {
            scale_results: HashMap::new(),
        }
    }

    pub fn calculate<T: HasPnl>(
        &mut self,
        trade_history: &[T],
        scale: Option<&str>,
    ) -> KellyResult {
        // Filter by scale if provided
        let trades: Vec<&T> = if let Some(s) = scale {
            trade_history
                .iter()
                .filter(|t| t.reason().contains(s))
                .collect()
        } else {
            trade_history.iter().collect()
        };

        // Apply rolling window
        let trades: Vec<&T> = if trades.len() > ROLLING_WINDOW {
            trades[trades.len() - ROLLING_WINDOW..].to_vec()
        } else {
            trades
        };

        // Not enough data
        if trades.len() < MIN_SAMPLE_SIZE {
            let result = KellyResult {
                full_kelly: 0.0,
                applied_fraction: DEFAULT_FRACTION,
                win_rate: 0.0,
                loss_rate: 0.0,
                payoff_ratio: 0.0,
                sample_size: trades.len(),
                using_default: true,
                edge: 0.0,
            };
            if let Some(s) = scale {
                self.scale_results.insert(s.to_string(), result.clone());
            }
            return result;
        }

        let total = trades.len() as f64;
        let wins: Vec<&&T> = trades.iter().filter(|t| t.pnl() > 0.0).collect();
        let losses: Vec<&&T> = trades.iter().filter(|t| t.pnl() <= 0.0).collect();

        let p = wins.len() as f64 / total;
        let q = 1.0 - p;

        let avg_win = if !wins.is_empty() {
            wins.iter().map(|t| t.pnl()).sum::<f64>() / wins.len() as f64
        } else {
            0.0
        };

        let avg_loss = if !losses.is_empty() {
            (losses.iter().map(|t| t.pnl()).sum::<f64>() / losses.len() as f64).abs()
        } else {
            1.0
        };

        let b = if avg_loss > 0.0 {
            avg_win / avg_loss
        } else {
            0.0
        };

        let full_kelly = if b > 0.0 { (b * p - q) / b } else { 0.0 };

        let edge = b * p - q;

        let mut applied = full_kelly * KELLY_MULTIPLIER;

        if full_kelly <= 0.0 {
            applied = MIN_KELLY_FRACTION;
        } else {
            applied = applied.max(MIN_KELLY_FRACTION).min(MAX_KELLY_FRACTION);
        }

        let result = KellyResult {
            full_kelly: round6(full_kelly),
            applied_fraction: round6(applied),
            win_rate: round4(p),
            loss_rate: round4(q),
            payoff_ratio: round4(b),
            sample_size: trades.len(),
            using_default: false,
            edge: round4(edge),
        };

        if let Some(s) = scale {
            self.scale_results.insert(s.to_string(), result.clone());
        }

        result
    }

    pub fn get_risk_amount<T: HasPnl>(
        &mut self,
        balance: f64,
        trade_history: &[T],
        scale: Option<&str>,
    ) -> (f64, KellyResult) {
        let result = self.calculate(trade_history, scale);
        let risk_amount = (balance * result.applied_fraction * 100.0).round() / 100.0;
        (risk_amount, result)
    }

    pub fn get_all_scale_results(&self) -> &HashMap<String, KellyResult> {
        &self.scale_results
    }
}

fn round4(x: f64) -> f64 {
    (x * 10000.0).round() / 10000.0
}

fn round6(x: f64) -> f64 {
    (x * 1000000.0).round() / 1000000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestTrade {
        pnl_val: f64,
        reason_str: String,
    }

    impl HasPnl for TestTrade {
        fn pnl(&self) -> f64 {
            self.pnl_val
        }
        fn reason(&self) -> &str {
            &self.reason_str
        }
    }

    fn make_trades(pnls: &[f64]) -> Vec<TestTrade> {
        pnls.iter()
            .map(|&p| TestTrade {
                pnl_val: p,
                reason_str: "5m test".to_string(),
            })
            .collect()
    }

    #[test]
    fn default_fraction_when_few_trades() {
        let trades = make_trades(&[1.0, -0.5, 2.0]);
        let mut kc = KellyCriterion::new();
        let r = kc.calculate(&trades, None);
        assert!(r.using_default);
        assert!((r.applied_fraction - DEFAULT_FRACTION).abs() < 1e-9);
    }

    #[test]
    fn known_sequence_kelly() {
        // 20 trades: 14 wins of +2.0, 6 losses of -1.0
        // WR = 0.7, avg_win=2, avg_loss=1, b=2, full_kelly = (2*0.7-0.3)/2 = 0.55
        // applied = 0.55 * 0.5 = 0.275, clamped to MAX=0.06
        let mut pnls = vec![2.0; 14];
        pnls.extend(vec![-1.0; 6]);
        let trades = make_trades(&pnls);
        let mut kc = KellyCriterion::new();
        let r = kc.calculate(&trades, None);
        assert!(!r.using_default);
        assert!((r.applied_fraction - MAX_KELLY_FRACTION).abs() < 1e-6);
    }

    #[test]
    fn min_clamp_when_negative_edge() {
        // All losses
        let trades = make_trades(&vec![-1.0; 25]);
        let mut kc = KellyCriterion::new();
        let r = kc.calculate(&trades, None);
        assert!((r.applied_fraction - MIN_KELLY_FRACTION).abs() < 1e-6);
    }

    #[test]
    fn rolling_window_trims() {
        // 150 trades, should only use last 100
        let mut pnls = vec![-5.0; 50]; // first 50 bad
        pnls.extend(vec![1.0; 100]);   // last 100 wins
        let trades = make_trades(&pnls);
        let mut kc = KellyCriterion::new();
        let r = kc.calculate(&trades, None);
        // Last 100 are all wins => WR=1.0, should have positive kelly
        assert!(!r.using_default);
        assert!(r.full_kelly > 0.0);
        assert_eq!(r.sample_size, 100);
    }

    #[test]
    fn get_risk_amount_correct() {
        let trades = make_trades(&vec![1.0; 5]); // too few, uses default
        let mut kc = KellyCriterion::new();
        let (risk, result) = kc.get_risk_amount(1000.0, &trades, None);
        assert!(result.using_default);
        let expected = (1000.0 * DEFAULT_FRACTION * 100.0).round() / 100.0;
        assert!((risk - expected).abs() < 0.01);
    }
}
