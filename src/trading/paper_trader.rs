use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::config::Config;
use crate::core::kelly::{HasPnl, KellyCriterion, KellyResult};
use crate::models::{Direction, PositionStatus};
use crate::strategies::signals::TradeSignal;
use crate::trading::trade_record::{TradeMetadata, TradeRecord};

/// Partial TP allocation — conservative (non-CISD)
const TP_ALLOC_CONSERVATIVE: &[(f64, f64)] = &[
    (-1.0, 0.60),
    (-2.0, 0.20),
    (-4.0, 0.10),
    (-4.5, 0.10),
];

/// Partial TP allocation — aggressive (CISD confirmed, let runners run)
const TP_ALLOC_AGGRESSIVE: &[(f64, f64)] = &[
    (-1.0, 0.10),
    (-2.0, 0.15),
    (-4.0, 0.30),
    (-4.5, 0.45),
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TpTarget {
    pub level: f64,
    pub price: f64,
    pub pct: f64,
    pub size_btc: f64,
    pub hit: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartialExit {
    pub level: f64,
    pub price: f64,
    pub size_btc: f64,
    pub pnl: f64,
    pub time: String,
    #[serde(default)]
    pub logged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub id: u64,
    pub direction: Direction,
    pub entry_price: f64,
    pub size_usd: f64,
    pub size_btc: f64,
    pub stop_loss: f64,
    pub take_profit: f64,
    pub entry_time: String,
    pub reason: String,
    #[serde(default)]
    pub kelly_fraction: f64,
    pub status: PositionStatus,
    #[serde(default)]
    pub exit_price: Option<f64>,
    #[serde(default)]
    pub exit_time: Option<String>,
    #[serde(default)]
    pub pnl: f64,
    #[serde(default)]
    pub remaining_size_btc: f64,
    #[serde(default)]
    pub tp_targets: Vec<TpTarget>,
    #[serde(default)]
    pub partial_exits: Vec<PartialExit>,
}

impl HasPnl for Position {
    fn pnl(&self) -> f64 {
        self.pnl
    }
    fn reason(&self) -> &str {
        &self.reason
    }
}

pub struct PaperTrader {
    pub balance: f64,
    pub positions: Vec<Position>,
    pub trade_history: Vec<Position>,
    pub trade_counter: u64,
    pub daily_pnl: f64,
    pub daily_pnl_date: String,
    pub kelly: KellyCriterion,
    pub last_kelly_result: Option<KellyResult>,
    pub trade_records: HashMap<u64, TradeRecord>,
    trades_file: String,
    records_file: String,
    /// When set, used instead of Utc::now() for timestamps (backtesting)
    pub sim_time: Option<DateTime<Utc>>,
    /// Trading fees as fraction (e.g., 0.001 = 0.1%)
    fee_rate: f64,
    /// Slippage as fraction (e.g., 0.0005 = 0.05%)
    slippage_rate: f64,
}

impl PaperTrader {
    pub fn new(cfg: &Config) -> Self {
        let mut trader = Self {
            balance: cfg.initial_balance,
            positions: Vec::new(),
            trade_history: Vec::new(),
            trade_counter: 0,
            daily_pnl: 0.0,
            daily_pnl_date: String::new(),
            kelly: KellyCriterion::new(),
            last_kelly_result: None,
            trade_records: HashMap::new(),
            trades_file: format!("{}/paper_trades.json", cfg.log_dir),
            records_file: format!("{}/trade_records.json", cfg.log_dir),
            sim_time: None,
            fee_rate: cfg.fee_rate,
            slippage_rate: cfg.slippage_rate,
        };
        trader.load_state(cfg);
        trader
    }

    /// Create a fresh trader without loading previous state (for backtesting)
    pub fn new_fresh(cfg: &Config) -> Self {
        Self {
            balance: cfg.initial_balance,
            positions: Vec::new(),
            trade_history: Vec::new(),
            trade_counter: 0,
            daily_pnl: 0.0,
            daily_pnl_date: String::new(),
            kelly: KellyCriterion::new(),
            last_kelly_result: None,
            trade_records: HashMap::new(),
            trades_file: String::new(),
            records_file: String::new(),
            sim_time: None,
            fee_rate: cfg.fee_rate,
            slippage_rate: cfg.slippage_rate,
        }
    }

    /// Get the current time (sim_time for backtesting, Utc::now() for live)
    fn now(&self) -> DateTime<Utc> {
        self.sim_time.unwrap_or_else(Utc::now)
    }

    pub fn can_open_position(&self, cfg: &Config) -> bool {
        let open_count = self
            .positions
            .iter()
            .filter(|p| p.status == PositionStatus::Open)
            .count();
        if open_count >= cfg.max_open_positions {
            return false;
        }

        let today = self.now().format("%Y-%m-%d").to_string();
        // Note: daily_pnl is checked against current state
        if self.daily_pnl_date == today
            && self.daily_pnl <= -(cfg.max_daily_loss * self.balance)
        {
            return false;
        }

        true
    }

    pub fn open_position(
        &mut self,
        signal: &TradeSignal,
        scale: &str,
        metadata: Option<TradeMetadata>,
    ) -> Option<&Position> {
        let sl_distance = (signal.entry_price - signal.stop_loss).abs();
        if sl_distance == 0.0 {
            return None;
        }

        // Kelly position sizing
        let (risk_amount, kelly_result) =
            self.kelly
                .get_risk_amount(self.balance, &self.trade_history, Some(scale));
        self.last_kelly_result = Some(kelly_result.clone());

        // Hard cap: max risk per trade (configurable via MAX_RISK_PCT env)
        let risk_pct: f64 = std::env::var("MAX_RISK_PCT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.02);
        let max_risk = self.balance * risk_pct;
        let capped_risk = risk_amount.min(max_risk);

        let mut size_btc = capped_risk / sl_distance;
        let mut size_usd = size_btc * signal.entry_price;

        // Leverage cap (configurable via MAX_LEVERAGE env, default 5x)
        let max_leverage: f64 = std::env::var("MAX_LEVERAGE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5.0);
        let max_position_usd = self.balance * max_leverage;
        if size_usd > max_position_usd {
            size_usd = max_position_usd;
            size_btc = size_usd / signal.entry_price;
        }

        // Apply entry fee + slippage
        let entry_fee = size_usd * self.fee_rate;
        let slippage_cost = size_usd * self.slippage_rate;
        self.balance -= entry_fee + slippage_cost;

        // Adjust entry price for slippage (adverse direction)
        let entry_price = match signal.direction {
            Direction::Long => signal.entry_price * (1.0 + self.slippage_rate),
            Direction::Short => signal.entry_price * (1.0 - self.slippage_rate),
        };

        self.trade_counter += 1;
        let id = self.trade_counter;

        // Build TP targets from SD levels — dynamic allocation based on CISD
        let tp_alloc = if signal.cisd_confirmed {
            TP_ALLOC_AGGRESSIVE
        } else {
            TP_ALLOC_CONSERVATIVE
        };
        let mut tp_targets = Vec::new();
        if let Some(ref tp_levels) = signal.tp_levels {
            let tp_map: HashMap<i64, f64> = tp_levels
                .iter()
                .filter_map(|l| l.level.map(|lv| ((lv * 10.0) as i64, l.price)))
                .collect();

            for &(level, pct) in tp_alloc {
                let key = (level * 10.0) as i64;
                if let Some(&price) = tp_map.get(&key) {
                    tp_targets.push(TpTarget {
                        level,
                        price,
                        pct,
                        size_btc: round8(size_btc * pct),
                        hit: false,
                    });
                }
            }
        }

        let pos = Position {
            id,
            direction: signal.direction,
            entry_price,
            size_usd: round2(size_usd),
            size_btc: round8(size_btc),
            stop_loss: signal.stop_loss,
            take_profit: signal.take_profit,
            entry_time: self.now().to_rfc3339(),
            reason: signal.reason.clone(),
            kelly_fraction: kelly_result.applied_fraction,
            status: PositionStatus::Open,
            exit_price: None,
            exit_time: None,
            pnl: 0.0,
            remaining_size_btc: round8(size_btc),
            tp_targets,
            partial_exits: Vec::new(),
        };

        self.positions.push(pos);

        // Trade record
        if let Some(mut md) = metadata {
            md.kelly_fraction = kelly_result.applied_fraction;
            self.trade_records.insert(
                id,
                TradeRecord {
                    position_id: id,
                    metadata: md,
                    outcome: String::new(),
                    pnl: 0.0,
                    hold_duration_seconds: 0.0,
                },
            );
        }

        self.save_state();
        self.positions.last()
    }

    pub fn check_positions(&mut self, current_price: f64) -> Vec<Position> {
        let mut closed = Vec::new();
        let mut changed = false;

        let mut i = 0;
        while i < self.positions.len() {
            if self.positions[i].status != PositionStatus::Open {
                i += 1;
                continue;
            }

            // Check SL
            let hit_sl = match self.positions[i].direction {
                Direction::Long => current_price <= self.positions[i].stop_loss,
                Direction::Short => current_price >= self.positions[i].stop_loss,
            };

            if hit_sl {
                // Exit at stop loss price (simulating stop order fill)
                self.close_position(i, self.positions[i].stop_loss, PositionStatus::ClosedSl);
                closed.push(self.positions[i].clone());
                changed = true;
                i += 1;
                continue;
            }

            // Check partial TP targets
            if !self.positions[i].tp_targets.is_empty() {
                let mut any_hit = false;
                for t_idx in 0..self.positions[i].tp_targets.len() {
                    if self.positions[i].tp_targets[t_idx].hit {
                        continue;
                    }
                    let hit = match self.positions[i].direction {
                        Direction::Long => {
                            current_price >= self.positions[i].tp_targets[t_idx].price
                        }
                        Direction::Short => {
                            current_price <= self.positions[i].tp_targets[t_idx].price
                        }
                    };
                    if hit {
                        self.partial_close(i, t_idx, current_price);
                        any_hit = true;
                        changed = true;
                    }
                }

                if any_hit {
                    // Check if all targets hit
                    let all_hit = self.positions[i].tp_targets.iter().all(|t| t.hit);
                    if all_hit {
                        if self.positions[i].remaining_size_btc > 0.0 {
                            self.close_position(i, current_price, PositionStatus::ClosedTp);
                        } else {
                            self.finalize_position(i, PositionStatus::ClosedTp);
                        }
                        closed.push(self.positions[i].clone());
                    }
                }
            } else {
                // No partial targets — single TP
                let hit_tp = match self.positions[i].direction {
                    Direction::Long => current_price >= self.positions[i].take_profit,
                    Direction::Short => current_price <= self.positions[i].take_profit,
                };
                if hit_tp {
                    self.close_position(i, current_price, PositionStatus::ClosedTp);
                    closed.push(self.positions[i].clone());
                    changed = true;
                }
            }

            i += 1;
        }

        if changed || !closed.is_empty() {
            self.save_state();
        }

        closed
    }

    fn partial_close(&mut self, pos_idx: usize, target_idx: usize, exit_price: f64) {
        let now_str = self.now().to_rfc3339();
        let fee_rate = self.fee_rate;
        let pos = &mut self.positions[pos_idx];
        let close_size = pos.tp_targets[target_idx]
            .size_btc
            .min(pos.remaining_size_btc);
        if close_size <= 0.0 {
            return;
        }

        let pnl = match pos.direction {
            Direction::Long => (exit_price - pos.entry_price) * close_size,
            Direction::Short => (pos.entry_price - exit_price) * close_size,
        };
        // Deduct exit fee
        let exit_fee = close_size * exit_price * fee_rate;
        let pnl = round2(pnl - exit_fee);

        pos.remaining_size_btc = round8(pos.remaining_size_btc - close_size);
        pos.pnl = round2(pos.pnl + pnl);
        self.balance += pnl;
        self.daily_pnl += pnl;

        pos.tp_targets[target_idx].hit = true;
        pos.partial_exits.push(PartialExit {
            level: pos.tp_targets[target_idx].level,
            price: exit_price,
            size_btc: close_size,
            pnl,
            time: now_str,
            logged: false,
        });

    }

    fn finalize_position(&mut self, pos_idx: usize, status: PositionStatus) {
        let now_str = self.now().to_rfc3339();
        let pos = &mut self.positions[pos_idx];
        pos.exit_price = pos.partial_exits.last().map(|pe| pe.price);
        pos.exit_time = Some(now_str);
        pos.status = status;

        let closed_pos = pos.clone();
        self.trade_history.push(closed_pos);

        self.update_trade_record(pos_idx);
    }

    fn close_position(&mut self, pos_idx: usize, exit_price: f64, status: PositionStatus) {
        let now_str = self.now().to_rfc3339();
        let fee_rate = self.fee_rate;
        let pos = &mut self.positions[pos_idx];
        let close_size = if pos.remaining_size_btc > 0.0 {
            pos.remaining_size_btc
        } else {
            pos.size_btc
        };

        let pnl = match pos.direction {
            Direction::Long => (exit_price - pos.entry_price) * close_size,
            Direction::Short => (pos.entry_price - exit_price) * close_size,
        };
        // Deduct exit fee
        let exit_fee = close_size * exit_price * fee_rate;
        let pnl = pnl - exit_fee;

        pos.exit_price = Some(exit_price);
        pos.exit_time = Some(now_str);
        pos.status = status;
        pos.pnl = round2(pos.pnl + pnl);
        pos.remaining_size_btc = 0.0;

        self.balance += pnl;
        self.daily_pnl += pnl;

        let closed_pos = pos.clone();
        self.trade_history.push(closed_pos);

        self.update_trade_record(pos_idx);
    }

    fn update_trade_record(&mut self, pos_idx: usize) {
        let pos = &self.positions[pos_idx];
        if let Some(record) = self.trade_records.get_mut(&pos.id) {
            record.outcome = if pos.pnl > 0.0 {
                "win".to_string()
            } else {
                "loss".to_string()
            };
            record.pnl = pos.pnl;

            if let (Ok(entry_dt), Some(exit_time)) = (
                DateTime::parse_from_rfc3339(&pos.entry_time),
                pos.exit_time.as_ref(),
            ) {
                if let Ok(exit_dt) = DateTime::parse_from_rfc3339(exit_time) {
                    record.hold_duration_seconds =
                        (exit_dt - entry_dt).num_seconds() as f64;
                }
            }
        }
    }

    pub fn get_stats(&mut self) -> TradingStats {
        let kelly = self.kelly.calculate(&self.trade_history, None);
        let open_count = self
            .positions
            .iter()
            .filter(|p| p.status == PositionStatus::Open)
            .count();

        if self.trade_history.is_empty() {
            return TradingStats {
                total_trades: 0,
                balance: self.balance,
                win_rate: 0.0,
                total_pnl: 0.0,
                avg_win: 0.0,
                avg_loss: 0.0,
                best_trade: 0.0,
                worst_trade: 0.0,
                open_positions: open_count,
                kelly_fraction: kelly.applied_fraction,
                kelly_full: kelly.full_kelly,
                kelly_using_default: kelly.using_default,
                kelly_edge: kelly.edge,
                kelly_sample: kelly.sample_size,
                kelly_win_rate: kelly.win_rate,
                kelly_payoff: kelly.payoff_ratio,
            };
        }

        let wins: Vec<&Position> = self.trade_history.iter().filter(|t| t.pnl > 0.0).collect();
        let losses: Vec<&Position> = self.trade_history.iter().filter(|t| t.pnl <= 0.0).collect();

        TradingStats {
            total_trades: self.trade_history.len(),
            balance: round2(self.balance),
            win_rate: round1(wins.len() as f64 / self.trade_history.len() as f64 * 100.0),
            total_pnl: round2(self.trade_history.iter().map(|t| t.pnl).sum()),
            avg_win: if wins.is_empty() {
                0.0
            } else {
                round2(wins.iter().map(|t| t.pnl).sum::<f64>() / wins.len() as f64)
            },
            avg_loss: if losses.is_empty() {
                0.0
            } else {
                round2(losses.iter().map(|t| t.pnl).sum::<f64>() / losses.len() as f64)
            },
            best_trade: round2(
                self.trade_history
                    .iter()
                    .map(|t| t.pnl)
                    .fold(f64::NEG_INFINITY, f64::max),
            ),
            worst_trade: round2(
                self.trade_history
                    .iter()
                    .map(|t| t.pnl)
                    .fold(f64::INFINITY, f64::min),
            ),
            open_positions: open_count,
            kelly_fraction: kelly.applied_fraction,
            kelly_full: kelly.full_kelly,
            kelly_using_default: kelly.using_default,
            kelly_edge: kelly.edge,
            kelly_sample: kelly.sample_size,
            kelly_win_rate: kelly.win_rate,
            kelly_payoff: kelly.payoff_ratio,
        }
    }

    pub fn get_kelly_by_scale(&mut self) -> HashMap<String, KellyResult> {
        let mut results = HashMap::new();
        for scale in &["1m", "5m", "15m"] {
            let kr = self.kelly.calculate(&self.trade_history, Some(scale));
            results.insert(scale.to_string(), kr);
        }
        results
    }

    fn save_state(&self) {
        let _ = fs::create_dir_all(Path::new(&self.trades_file).parent().unwrap_or(Path::new("logs")));

        let state = serde_json::json!({
            "balance": self.balance,
            "trade_counter": self.trade_counter,
            "daily_pnl": self.daily_pnl,
            "daily_pnl_date": self.daily_pnl_date,
            "positions": self.positions,
            "trade_history": self.trade_history,
        });

        if let Ok(json) = serde_json::to_string_pretty(&state) {
            let _ = fs::write(&self.trades_file, json);
        }

        if !self.trade_records.is_empty() {
            if let Ok(json) = serde_json::to_string_pretty(&self.trade_records) {
                let _ = fs::write(&self.records_file, json);
            }
        }
    }

    fn load_state(&mut self, cfg: &Config) {
        if let Ok(content) = fs::read_to_string(&self.trades_file) {
            if let Ok(state) = serde_json::from_str::<serde_json::Value>(&content) {
                self.balance = state["balance"].as_f64().unwrap_or(cfg.initial_balance);
                self.trade_counter = state["trade_counter"].as_u64().unwrap_or(0);
                self.daily_pnl = state["daily_pnl"].as_f64().unwrap_or(0.0);
                self.daily_pnl_date = state["daily_pnl_date"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();

                if let Ok(positions) =
                    serde_json::from_value::<Vec<Position>>(state["positions"].clone())
                {
                    self.positions = positions;
                }
                if let Ok(history) =
                    serde_json::from_value::<Vec<Position>>(state["trade_history"].clone())
                {
                    self.trade_history = history;
                }
            }
        }

        if let Ok(content) = fs::read_to_string(&self.records_file) {
            if let Ok(records) =
                serde_json::from_str::<HashMap<u64, TradeRecord>>(&content)
            {
                self.trade_records = records;
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TradingStats {
    pub total_trades: usize,
    pub balance: f64,
    pub win_rate: f64,
    pub total_pnl: f64,
    pub avg_win: f64,
    pub avg_loss: f64,
    pub best_trade: f64,
    pub worst_trade: f64,
    pub open_positions: usize,
    pub kelly_fraction: f64,
    pub kelly_full: f64,
    pub kelly_using_default: bool,
    pub kelly_edge: f64,
    pub kelly_sample: usize,
    pub kelly_win_rate: f64,
    pub kelly_payoff: f64,
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}
fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}
fn round8(x: f64) -> f64 {
    (x * 1e8).round() / 1e8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::default_test_config;

    fn test_config() -> Config {
        let mut cfg = default_test_config();
        // Use a unique temp dir for each test to avoid state leaking
        cfg.log_dir = std::env::temp_dir()
            .join(format!("ict_bot_test_{}", std::process::id()))
            .to_string_lossy()
            .to_string();
        cfg
    }

    fn make_signal(direction: Direction, entry: f64, sl: f64, tp: f64) -> TradeSignal {
        TradeSignal {
            direction,
            entry_price: entry,
            stop_loss: sl,
            take_profit: tp,
            pda_engaged: None,
            cisd_confirmed: false,
            confidence: 0.7,
            session: "london".to_string(),
            session_weight: 1.5,
            reason: "test signal 5m".to_string(),
            tp_levels: None,
        }
    }

    #[test]
    fn open_position_creates_correctly() {
        let cfg = test_config();
        let mut trader = PaperTrader::new(&cfg);
        let signal = make_signal(Direction::Long, 50000.0, 49500.0, 51000.0);
        let pos = trader.open_position(&signal, "5m", None);
        assert!(pos.is_some());
        let pos = pos.unwrap();
        assert_eq!(pos.direction, Direction::Long);
        assert!((pos.entry_price - 50000.0).abs() < 0.01);
        assert_eq!(pos.status, PositionStatus::Open);
        assert!(pos.size_btc > 0.0);
        assert!(pos.size_usd > 0.0);
    }

    #[test]
    fn check_positions_sl_hit_long() {
        let cfg = test_config();
        let mut trader = PaperTrader::new(&cfg);
        let signal = make_signal(Direction::Long, 50000.0, 49500.0, 51000.0);
        trader.open_position(&signal, "5m", None);

        let closed = trader.check_positions(49400.0); // below SL
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].status, PositionStatus::ClosedSl);
        assert!(closed[0].pnl < 0.0);
    }

    #[test]
    fn check_positions_tp_hit_long() {
        let cfg = test_config();
        let mut trader = PaperTrader::new(&cfg);
        let signal = make_signal(Direction::Long, 50000.0, 49500.0, 51000.0);
        trader.open_position(&signal, "5m", None);

        let closed = trader.check_positions(51100.0); // above TP
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].status, PositionStatus::ClosedTp);
        assert!(closed[0].pnl > 0.0);
    }

    #[test]
    fn check_positions_sl_hit_short() {
        let cfg = test_config();
        let mut trader = PaperTrader::new(&cfg);
        let signal = make_signal(Direction::Short, 50000.0, 50500.0, 49000.0);
        trader.open_position(&signal, "5m", None);

        let closed = trader.check_positions(50600.0); // above SL
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].status, PositionStatus::ClosedSl);
    }

    #[test]
    fn can_open_position_respects_max() {
        let cfg = test_config();
        let mut trader = PaperTrader::new(&cfg);
        // Open max_open_positions (3)
        for _ in 0..3 {
            let signal = make_signal(Direction::Long, 50000.0, 49500.0, 51000.0);
            trader.open_position(&signal, "5m", None);
        }
        assert!(!trader.can_open_position(&cfg));
    }

    #[test]
    fn balance_updates_on_close() {
        let cfg = test_config();
        let mut trader = PaperTrader::new(&cfg);
        let initial_balance = trader.balance;
        let signal = make_signal(Direction::Long, 50000.0, 49500.0, 51000.0);
        trader.open_position(&signal, "5m", None);

        // Trigger TP hit
        let closed = trader.check_positions(51100.0);
        assert!(!closed.is_empty());
        // Balance should have increased
        assert!(trader.balance > initial_balance);
    }
}
