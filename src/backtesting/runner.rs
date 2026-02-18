use anyhow::Result;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::collections::HashMap;
use tracing::{debug, info};

use crate::config::Config;
use crate::core::sessions::SessionManager;
use crate::core::stop_loss::StopLossEngine;
use crate::exchange::{Exchange, HistoricalExchange};
use crate::models::{CandleSeries, Direction, PositionStatus, Timeframe};
use crate::strategies::fractal_engine::FractalEngine;
use crate::strategies::weekly_profiles::{WeeklyBias, WeeklyProfileClassifier};
use crate::trading::paper_trader::PaperTrader;
use crate::trading::strategy_refiner::StrategyRefiner;
use crate::trading::trade_record::TradeMetadata;

use super::report::BacktestReport;

/// Steps through historical data candle-by-candle, running the full
/// ICT fractal engine + paper trader pipeline at each step.
pub struct BacktestRunner {
    pub exchange: HistoricalExchange,
    pub config: Config,
    pub paper_trader: PaperTrader,
    fractal: FractalEngine,
    session: SessionManager,
    weekly_classifier: WeeklyProfileClassifier,
    refiner: StrategyRefiner,
    weekly_bias: Option<WeeklyBias>,
    scale_positions: HashMap<String, u64>,
    scale_cooldown: HashMap<String, DateTime<Utc>>,
    data_cache: HashMap<Timeframe, CandleSeries>,

    // Counters
    total_signals: usize,
    signals_filtered: usize,
    last_weekly_ts: Option<DateTime<Utc>>,
}

impl BacktestRunner {
    pub fn new(exchange: HistoricalExchange, config: Config) -> Self {
        let fractal = FractalEngine::new(&config);
        let session = SessionManager::new(&config);
        let paper_trader = PaperTrader::new_fresh(&config);
        let refiner = StrategyRefiner::new(&config);

        Self {
            exchange,
            config: config.clone(),
            paper_trader,
            fractal,
            session,
            weekly_classifier: WeeklyProfileClassifier::new(),
            refiner,
            weekly_bias: None,
            scale_positions: HashMap::new(),
            scale_cooldown: HashMap::new(),
            data_cache: HashMap::new(),
            total_signals: 0,
            signals_filtered: 0,
            last_weekly_ts: None,
        }
    }

    /// Run the full backtest. Returns a report.
    pub async fn run(
        &mut self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        step_minutes: i64,
    ) -> Result<BacktestReport> {
        let step = ChronoDuration::minutes(step_minutes);
        let mut current = start;
        let total_steps = ((end - start).num_minutes() / step_minutes) as usize;
        let mut step_count = 0usize;
        let log_interval = total_steps / 20; // Log ~20 progress updates

        info!("=== BACKTEST START ===");
        info!(
            "Period: {} to {} ({} steps of {}m)",
            start.format("%Y-%m-%d"),
            end.format("%Y-%m-%d"),
            total_steps,
            step_minutes
        );
        info!("Initial balance: ${:.2}", self.config.initial_balance);

        let initial_balance = self.config.initial_balance;

        // Equity curve tracking
        let mut equity_curve: Vec<(DateTime<Utc>, f64)> = Vec::new();
        let mut max_equity = initial_balance;
        let mut max_drawdown = 0.0f64;
        let mut max_drawdown_pct = 0.0f64;

        while current <= end {
            self.exchange.set_time(current);
            self.paper_trader.sim_time = Some(current);
            step_count += 1;

            // Progress logging
            if log_interval > 0 && step_count % log_interval == 0 {
                let pct = (step_count as f64 / total_steps as f64) * 100.0;
                info!(
                    "  Progress: {:.0}% | {} | Balance: ${:.2} | Trades: {} | Signals: {}",
                    pct,
                    current.format("%Y-%m-%d %H:%M"),
                    self.paper_trader.balance,
                    self.paper_trader.trade_history.len(),
                    self.total_signals,
                );
            }

            // Refresh data cache
            self.refresh_data().await;

            // Update session (using simulated time)
            self.session.update(&self.config, Some(current));

            // Weekly profile analysis (every 4 hours of sim time)
            let should_analyze_weekly = match self.last_weekly_ts {
                Some(last) => (current - last).num_hours() >= 4,
                None => true,
            };
            if should_analyze_weekly {
                self.analyze_weekly();
                self.last_weekly_ts = Some(current);
            }

            // Check positions
            self.check_positions(current).await;

            // Scan all scales
            let scale_keys: Vec<String> = self.config.hft_scales.keys().cloned().collect();
            for scale_key in &scale_keys {
                self.scan_scale(scale_key, current).await;
            }

            // Track equity
            let equity = self.paper_trader.balance;
            equity_curve.push((current, equity));
            if equity > max_equity {
                max_equity = equity;
            }
            let dd = max_equity - equity;
            if dd > max_drawdown {
                max_drawdown = dd;
                max_drawdown_pct = if max_equity > 0.0 {
                    dd / max_equity * 100.0
                } else {
                    0.0
                };
            }

            current = current + step;
        }

        // Close any remaining open positions at the last known price
        if let Ok(price) = self.exchange.get_current_price().await {
            let _ = self.paper_trader.check_positions(price);
        }

        info!("=== BACKTEST COMPLETE ===");

        Ok(BacktestReport::from_backtest(
            &self.paper_trader,
            &self.config,
            start,
            end,
            equity_curve,
            max_drawdown,
            max_drawdown_pct,
            self.total_signals,
            self.signals_filtered,
        ))
    }

    async fn refresh_data(&mut self) {
        let timeframes = [
            (Timeframe::M1, 200usize),
            (Timeframe::M5, 200),
            (Timeframe::M15, 200),
            (Timeframe::H1, 200),
            (Timeframe::D1, 30),
        ];

        for (tf, limit) in timeframes {
            if let Ok(data) = self.exchange.fetch_ohlcv(tf, limit).await {
                if !data.is_empty() {
                    self.data_cache.insert(tf, data);
                }
            }
        }

        if let Ok(data) = self.exchange.get_4h(200).await {
            if !data.is_empty() {
                self.data_cache.insert(Timeframe::H4, data);
            }
        }
    }

    fn analyze_weekly(&mut self) {
        let daily = match self.data_cache.get(&Timeframe::D1) {
            Some(d) if !d.is_empty() => d,
            _ => return,
        };
        let htf = match self.data_cache.get(&Timeframe::H1) {
            Some(d) if !d.is_empty() => d,
            _ => return,
        };

        let day = self.session.get_day_of_week();
        let bias = self.weekly_classifier.classify(daily, htf, &day, &self.config);
        self.weekly_bias = Some(bias);
    }

    async fn check_positions(&mut self, sim_time: DateTime<Utc>) {
        let open_pos: Vec<(usize, Direction, f64)> = self
            .paper_trader
            .positions
            .iter()
            .enumerate()
            .filter(|(_, p)| p.status == PositionStatus::Open)
            .map(|(i, p)| (i, p.direction, p.stop_loss))
            .collect();

        if open_pos.is_empty() {
            return;
        }

        let current_price = match self.exchange.get_current_price().await {
            Ok(p) => p,
            Err(_) => return,
        };

        // Trail stops
        for &(_, direction, stop_loss) in &open_pos {
            if let Some(trail_df) = self.data_cache.get(&Timeframe::M5) {
                let mut trail_engine = StopLossEngine::new();
                if let Some(new_sl) =
                    trail_engine.get_trailing_stop(direction, stop_loss, trail_df, None)
                {
                    for pos in &mut self.paper_trader.positions {
                        if pos.status == PositionStatus::Open
                            && pos.direction == direction
                            && (pos.stop_loss - stop_loss).abs() < 0.01
                        {
                            pos.stop_loss = new_sl.price;
                            break;
                        }
                    }
                }
            }
        }

        let closed = self.paper_trader.check_positions(current_price);

        for pos in &closed {
            let result = if pos.pnl > 0.0 { "WIN" } else { "LOSS" };
            debug!(
                "[BT {}] Position #{} {} PnL ${:+.2}",
                sim_time.format("%m-%d %H:%M"),
                pos.id,
                result,
                pos.pnl,
            );

            // Remove from scale_positions and set cooldown
            let keys_to_remove: Vec<String> = self
                .scale_positions
                .iter()
                .filter(|(_, &pid)| pid == pos.id)
                .map(|(k, _)| k.clone())
                .collect();
            for key in keys_to_remove {
                self.scale_positions.remove(&key);
                // 30-minute cooldown after position closes to prevent churning
                self.scale_cooldown.insert(key, sim_time + ChronoDuration::minutes(30));
            }
        }
    }

    async fn scan_scale(&mut self, scale_key: &str, sim_time: DateTime<Utc>) {
        let weekly_bias = match &self.weekly_bias {
            Some(b) => b.clone(),
            None => return,
        };

        let day = self.session.get_day_of_week();
        if day == "Monday" {
            return;
        }

        // Only trade during killzones (London, NY Forex, NY Indices)
        if !self.session.is_killzone() {
            return;
        }

        let profile_str = weekly_bias.profile.to_string();
        if !self.session.should_trade_today(&self.config, &profile_str) {
            return;
        }

        if self.scale_positions.contains_key(scale_key) {
            return;
        }

        // Cooldown after position close to prevent churning
        if let Some(&cooldown_until) = self.scale_cooldown.get(scale_key) {
            if sim_time < cooldown_until {
                return;
            }
            self.scale_cooldown.remove(&scale_key.to_string());
        }

        if !self.paper_trader.can_open_position(&self.config) {
            return;
        }

        if self.data_cache.is_empty() {
            return;
        }

        if self.refiner.should_skip(scale_key, &self.session.current_session) {
            return;
        }

        let midnight_open = self.exchange.get_midnight_open().await.ok().flatten();

        // Evaluate this scale
        let scale = match self.fractal.scales.get_mut(scale_key) {
            Some(s) => s,
            None => return,
        };

        let signal = match scale.evaluate(&self.data_cache, midnight_open, &self.session, &self.config)
        {
            Some(s) => s,
            None => return,
        };

        self.total_signals += 1;

        // Cross-scale confluence
        let all_signals =
            self.fractal
                .evaluate_all(&self.data_cache, midnight_open, &self.session, &self.config);

        let signal = all_signals
            .into_iter()
            .find(|s| s.scale == scale_key)
            .unwrap_or(signal);

        let min_conf = self.config.hft_scales[scale_key].min_confidence;
        if signal.confidence < min_conf {
            self.signals_filtered += 1;
            return;
        }

        // Minimum TP distance filter: ensure expected profit > round-trip fees
        let tp_dist_pct = (signal.take_profit - signal.entry_price).abs() / signal.entry_price;
        let round_trip_fee = (self.config.fee_rate + self.config.slippage_rate) * 2.0;
        let min_tp_multiple: f64 = std::env::var("MIN_TP_MULTIPLE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3.0);
        if tp_dist_pct < round_trip_fee * min_tp_multiple {
            self.signals_filtered += 1;
            return;
        }

        // Build metadata
        let pda = &signal.pda_engaged;
        let metadata = TradeMetadata {
            scale: scale_key.to_string(),
            direction: signal.direction.to_string(),
            confidence: signal.confidence,
            session: signal.session.clone(),
            session_weight: signal.session_weight,
            cisd_confirmed: signal.cisd_confirmed,
            pda_type: pda.pda_type.to_string(),
            pda_direction: pda.direction.to_string(),
            pda_zone: pda.zone.to_string(),
            pda_strength: pda.strength,
            stop_mode: signal.stop_mode.clone(),
            tp_label: signal.tp_label.clone(),
            tp_levels: signal.tp_levels.clone(),
            cross_scale_confluence: signal.cross_scale_confluence,
            alignment: signal.alignment.clone(),
            weekly_profile: weekly_bias.profile.to_string(),
            weekly_direction: weekly_bias.direction.to_string(),
            weekly_confidence: weekly_bias.confidence,
            day_of_week: day,
            kelly_fraction: 0.0,
        };

        let trade_signal = signal.to_trade_signal();
        if let Some(pos) = self.paper_trader.open_position(&trade_signal, scale_key, Some(metadata))
        {
            let pos_id = pos.id;
            self.scale_positions.insert(scale_key.to_string(), pos_id);

            debug!(
                "[BT {}] Signal {} {} conf={:.0}% -> Position #{}",
                sim_time.format("%m-%d %H:%M"),
                scale_key,
                signal.direction,
                signal.confidence * 100.0,
                pos_id,
            );
        }
    }
}
