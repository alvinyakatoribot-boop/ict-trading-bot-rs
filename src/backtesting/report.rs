use chrono::{DateTime, Utc};
use std::collections::HashMap;

use crate::config::Config;
use crate::trading::paper_trader::PaperTrader;

#[derive(Debug, Clone)]
pub struct BacktestReport {
    // Period
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub days: f64,

    // Performance
    pub initial_balance: f64,
    pub final_balance: f64,
    pub total_pnl: f64,
    pub total_return_pct: f64,

    // Trades
    pub total_trades: usize,
    pub winning_trades: usize,
    pub losing_trades: usize,
    pub win_rate: f64,
    pub avg_win: f64,
    pub avg_loss: f64,
    pub profit_factor: f64,
    pub best_trade: f64,
    pub worst_trade: f64,
    pub avg_trade: f64,

    // Risk
    pub max_drawdown: f64,
    pub max_drawdown_pct: f64,
    pub sharpe_ratio: f64,

    // Signals
    pub total_signals: usize,
    pub signals_filtered: usize,

    // By scale
    pub scale_stats: HashMap<String, ScaleStats>,

    // By session
    pub session_stats: HashMap<String, SessionStats>,

    // Equity curve
    pub equity_curve: Vec<(DateTime<Utc>, f64)>,
}

#[derive(Debug, Clone, Default)]
pub struct ScaleStats {
    pub trades: usize,
    pub wins: usize,
    pub losses: usize,
    pub win_rate: f64,
    pub total_pnl: f64,
    pub avg_pnl: f64,
}

#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    pub trades: usize,
    pub wins: usize,
    pub losses: usize,
    pub win_rate: f64,
    pub total_pnl: f64,
}

impl BacktestReport {
    pub fn from_backtest(
        trader: &PaperTrader,
        cfg: &Config,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        equity_curve: Vec<(DateTime<Utc>, f64)>,
        max_drawdown: f64,
        max_drawdown_pct: f64,
        total_signals: usize,
        signals_filtered: usize,
    ) -> Self {
        let initial = cfg.initial_balance;
        let final_balance = trader.balance;
        let total_pnl = final_balance - initial;
        let days = (end - start).num_hours() as f64 / 24.0;

        let history = &trader.trade_history;
        let total_trades = history.len();

        let wins: Vec<f64> = history.iter().filter(|t| t.pnl > 0.0).map(|t| t.pnl).collect();
        let losses: Vec<f64> = history.iter().filter(|t| t.pnl <= 0.0).map(|t| t.pnl).collect();

        let winning = wins.len();
        let losing = losses.len();
        let win_rate = if total_trades > 0 {
            winning as f64 / total_trades as f64 * 100.0
        } else {
            0.0
        };

        let avg_win = if !wins.is_empty() {
            wins.iter().sum::<f64>() / wins.len() as f64
        } else {
            0.0
        };
        let avg_loss = if !losses.is_empty() {
            losses.iter().sum::<f64>() / losses.len() as f64
        } else {
            0.0
        };

        let profit_factor = if avg_loss.abs() > 0.0 {
            wins.iter().sum::<f64>() / losses.iter().sum::<f64>().abs()
        } else if !wins.is_empty() {
            f64::INFINITY
        } else {
            0.0
        };

        let best_trade = history
            .iter()
            .map(|t| t.pnl)
            .fold(f64::NEG_INFINITY, f64::max);
        let worst_trade = history
            .iter()
            .map(|t| t.pnl)
            .fold(f64::INFINITY, f64::min);
        let avg_trade = if total_trades > 0 {
            total_pnl / total_trades as f64
        } else {
            0.0
        };

        // Sharpe ratio (annualized, using daily returns from equity curve)
        let sharpe_ratio = compute_sharpe(&equity_curve);

        // Per-scale stats
        let mut scale_stats: HashMap<String, ScaleStats> = HashMap::new();
        for record in trader.trade_records.values() {
            let entry = scale_stats
                .entry(record.metadata.scale.clone())
                .or_default();
            entry.trades += 1;
            entry.total_pnl += record.pnl;
            if record.pnl > 0.0 {
                entry.wins += 1;
            } else {
                entry.losses += 1;
            }
        }
        for stats in scale_stats.values_mut() {
            stats.win_rate = if stats.trades > 0 {
                stats.wins as f64 / stats.trades as f64 * 100.0
            } else {
                0.0
            };
            stats.avg_pnl = if stats.trades > 0 {
                stats.total_pnl / stats.trades as f64
            } else {
                0.0
            };
        }

        // Per-session stats
        let mut session_stats: HashMap<String, SessionStats> = HashMap::new();
        for record in trader.trade_records.values() {
            let entry = session_stats
                .entry(record.metadata.session.clone())
                .or_default();
            entry.trades += 1;
            entry.total_pnl += record.pnl;
            if record.pnl > 0.0 {
                entry.wins += 1;
            } else {
                entry.losses += 1;
            }
        }
        for stats in session_stats.values_mut() {
            stats.win_rate = if stats.trades > 0 {
                stats.wins as f64 / stats.trades as f64 * 100.0
            } else {
                0.0
            };
        }

        BacktestReport {
            start,
            end,
            days,
            initial_balance: initial,
            final_balance,
            total_pnl,
            total_return_pct: if initial > 0.0 {
                total_pnl / initial * 100.0
            } else {
                0.0
            },
            total_trades,
            winning_trades: winning,
            losing_trades: losing,
            win_rate,
            avg_win,
            avg_loss,
            profit_factor,
            best_trade: if total_trades > 0 { best_trade } else { 0.0 },
            worst_trade: if total_trades > 0 { worst_trade } else { 0.0 },
            avg_trade,
            max_drawdown,
            max_drawdown_pct,
            sharpe_ratio,
            total_signals,
            signals_filtered,
            scale_stats,
            session_stats,
            equity_curve,
        }
    }

    pub fn print_summary(&self) {
        println!("\n{}", "=".repeat(70));
        println!("  BACKTEST REPORT");
        println!("{}", "=".repeat(70));
        println!(
            "  Period:      {} to {} ({:.0} days)",
            self.start.format("%Y-%m-%d"),
            self.end.format("%Y-%m-%d"),
            self.days
        );
        println!();
        println!("  PERFORMANCE");
        println!("  ───────────────────────────────────");
        println!("  Initial:     ${:.2}", self.initial_balance);
        println!("  Final:       ${:.2}", self.final_balance);
        println!("  PnL:         ${:+.2}", self.total_pnl);
        println!("  Return:      {:+.1}%", self.total_return_pct);
        println!();
        println!("  TRADES");
        println!("  ───────────────────────────────────");
        println!("  Total:       {}", self.total_trades);
        println!(
            "  Win/Loss:    {} / {}",
            self.winning_trades, self.losing_trades
        );
        println!("  Win Rate:    {:.1}%", self.win_rate);
        println!("  Avg Win:     ${:+.2}", self.avg_win);
        println!("  Avg Loss:    ${:+.2}", self.avg_loss);
        println!("  Best:        ${:+.2}", self.best_trade);
        println!("  Worst:       ${:+.2}", self.worst_trade);
        println!("  Avg Trade:   ${:+.2}", self.avg_trade);
        println!("  Profit Factor: {:.2}", self.profit_factor);
        println!();
        println!("  RISK");
        println!("  ───────────────────────────────────");
        println!("  Max DD:      ${:.2} ({:.1}%)", self.max_drawdown, self.max_drawdown_pct);
        println!("  Sharpe:      {:.2}", self.sharpe_ratio);
        println!();
        println!("  SIGNALS");
        println!("  ───────────────────────────────────");
        println!("  Generated:   {}", self.total_signals);
        println!("  Filtered:    {}", self.signals_filtered);
        println!(
            "  Conversion:  {:.1}%",
            if self.total_signals > 0 {
                self.total_trades as f64 / self.total_signals as f64 * 100.0
            } else {
                0.0
            }
        );

        if !self.scale_stats.is_empty() {
            println!();
            println!("  BY SCALE");
            println!("  ───────────────────────────────────");
            let mut scales: Vec<_> = self.scale_stats.iter().collect();
            scales.sort_by_key(|(k, _)| k.clone());
            for (scale, stats) in scales {
                println!(
                    "  {:>4}: {} trades | WR {:.0}% | PnL ${:+.2} | Avg ${:+.2}",
                    scale, stats.trades, stats.win_rate, stats.total_pnl, stats.avg_pnl
                );
            }
        }

        if !self.session_stats.is_empty() {
            println!();
            println!("  BY SESSION");
            println!("  ───────────────────────────────────");
            let mut sessions: Vec<_> = self.session_stats.iter().collect();
            sessions.sort_by(|a, b| b.1.total_pnl.partial_cmp(&a.1.total_pnl).unwrap());
            for (session, stats) in sessions {
                println!(
                    "  {:>12}: {} trades | WR {:.0}% | PnL ${:+.2}",
                    session, stats.trades, stats.win_rate, stats.total_pnl
                );
            }
        }

        println!("{}", "=".repeat(70));
    }
}

fn compute_sharpe(equity_curve: &[(DateTime<Utc>, f64)]) -> f64 {
    if equity_curve.len() < 2 {
        return 0.0;
    }

    // Compute daily returns (sample once per day)
    let mut daily_values: Vec<f64> = Vec::new();
    let mut last_day = None;
    for (ts, val) in equity_curve {
        let day = ts.date_naive();
        if last_day != Some(day) {
            daily_values.push(*val);
            last_day = Some(day);
        }
    }

    if daily_values.len() < 2 {
        return 0.0;
    }

    let returns: Vec<f64> = daily_values
        .windows(2)
        .map(|w| (w[1] - w[0]) / w[0])
        .collect();

    let n = returns.len() as f64;
    let mean = returns.iter().sum::<f64>() / n;
    let variance = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n;
    let std_dev = variance.sqrt();

    if std_dev == 0.0 {
        return 0.0;
    }

    // Annualized Sharpe (assuming ~252 trading days)
    mean / std_dev * 252.0_f64.sqrt()
}
