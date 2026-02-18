use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::time::Instant;
use tracing::{debug, error, info, warn};

use ict_trading_bot::config::{Config, SharedConfig};
use ict_trading_bot::core::sessions::SessionManager;
use ict_trading_bot::core::stop_loss::StopLossEngine;
use ict_trading_bot::exchange::Exchange;
use ict_trading_bot::models::{CandleSeries, Direction, PositionStatus, Timeframe};
use ict_trading_bot::strategies::fractal_engine::FractalEngine;
use ict_trading_bot::strategies::weekly_profiles::{WeeklyBias, WeeklyProfileClassifier};
use ict_trading_bot::trading::paper_trader::PaperTrader;
use ict_trading_bot::trading::strategy_refiner::StrategyRefiner;
use ict_trading_bot::trading::trade_record::TradeMetadata;

const WEEKLY_ANALYSIS_INTERVAL: f64 = 3600.0;
const POSITION_CHECK_INTERVAL: f64 = 10.0;
const ALIGNMENT_LOG_INTERVAL: f64 = 300.0;
const DATA_REFRESH_INTERVAL: f64 = 5.0;

pub struct IctBot {
    config: SharedConfig,
    market: Box<dyn Exchange>,
    session: SessionManager,
    weekly_classifier: WeeklyProfileClassifier,
    fractal: FractalEngine,
    paper_trader: PaperTrader,
    refiner: StrategyRefiner,

    last_weekly_analysis: Instant,
    last_position_check: Instant,
    last_alignment_log: Instant,
    last_data_refresh: Instant,
    last_analysis: Instant,
    closed_since_analysis: usize,
    weekly_bias: Option<WeeklyBias>,

    last_scan: HashMap<String, Instant>,
    scale_positions: HashMap<String, u64>,
    scale_cooldown: HashMap<String, DateTime<Utc>>,
    data_cache: HashMap<Timeframe, CandleSeries>,
}

impl IctBot {
    pub async fn new(config: SharedConfig, market: Box<dyn Exchange>) -> Self {
        let cfg = config.read().await;

        info!("{}", "=".repeat(60));
        info!("ICT HFT Bot starting up");
        info!(
            "Mode: {}",
            if cfg.paper_trade {
                "PAPER TRADING"
            } else {
                "LIVE TRADING"
            }
        );
        info!("Symbol: {}", cfg.symbol);
        info!("Entry scales:");
        for (_key, scale_cfg) in &cfg.hft_scales {
            let alignment_tfs: Vec<String> =
                scale_cfg.alignment_tfs.iter().map(|tf| tf.to_string()).collect();
            info!(
                "  {}: entry={} aligned={} scan={}s",
                scale_cfg.name,
                scale_cfg.entry_tf,
                alignment_tfs.join("+"),
                scale_cfg.scan_interval
            );
        }
        info!("{}", "=".repeat(60));

        let now = Instant::now();
        let last_scan: HashMap<String, Instant> = cfg
            .hft_scales
            .keys()
            .map(|k| (k.clone(), now))
            .collect();

        let session = SessionManager::new(&cfg);
        let fractal = FractalEngine::new(&cfg);
        let paper_trader = PaperTrader::new(&cfg);
        let refiner = StrategyRefiner::new(&cfg);

        drop(cfg);

        Self {
            config,
            market,
            session,
            weekly_classifier: WeeklyProfileClassifier::new(),
            fractal,
            paper_trader,
            refiner,
            last_weekly_analysis: now,
            last_position_check: now,
            last_alignment_log: now,
            last_data_refresh: now,
            last_analysis: now,
            closed_since_analysis: 0,
            weekly_bias: None,
            last_scan,
            scale_positions: HashMap::new(),
            scale_cooldown: HashMap::new(),
            data_cache: HashMap::new(),
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        info!("Bot is now running. Press Ctrl+C to stop.");
        self.print_status().await;

        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    self.shutdown().await;
                    return Ok(());
                }
                _ = self.tick() => {}
            }
        }
    }

    async fn tick(&mut self) {
        let cfg = self.config.read().await.clone();
        self.session.update(&cfg, None);

        // Weekly profile
        if self.last_weekly_analysis.elapsed().as_secs_f64() > WEEKLY_ANALYSIS_INTERVAL {
            self.analyze_weekly(&cfg);
            self.last_weekly_analysis = Instant::now();
        }

        // Refresh market data
        if self.last_data_refresh.elapsed().as_secs_f64() > DATA_REFRESH_INTERVAL {
            self.refresh_data().await;
            self.last_data_refresh = Instant::now();
        }

        // Check positions
        if self.last_position_check.elapsed().as_secs_f64() > POSITION_CHECK_INTERVAL {
            self.check_positions(&cfg).await;
            self.last_position_check = Instant::now();
        }

        // Alignment dashboard
        if self.last_alignment_log.elapsed().as_secs_f64() > ALIGNMENT_LOG_INTERVAL {
            self.log_alignment(&cfg);
            self.last_alignment_log = Instant::now();
        }

        // Scan each entry scale at its own interval
        let scale_keys: Vec<String> = cfg.hft_scales.keys().cloned().collect();
        for scale_key in &scale_keys {
            let interval = cfg.hft_scales[scale_key].scan_interval;
            let last = self.last_scan.get(scale_key).copied().unwrap_or(Instant::now());
            if last.elapsed().as_secs() >= interval {
                self.scan_scale(scale_key, &cfg).await;
                self.last_scan.insert(scale_key.clone(), Instant::now());
            }
        }

        // Self-learning analysis
        let analysis_interval = cfg.analysis_interval as f64;
        if self.last_analysis.elapsed().as_secs_f64() > analysis_interval
            || self.closed_since_analysis >= 10
        {
            self.run_analysis().await;
            self.last_analysis = Instant::now();
            self.closed_since_analysis = 0;
        }

        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }

    async fn refresh_data(&mut self) {
        let lookback: usize = std::env::var("DATA_LOOKBACK")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(175);
        let timeframes = [
            (Timeframe::M1, lookback),
            (Timeframe::M5, lookback),
            (Timeframe::M15, lookback),
            (Timeframe::H1, lookback),
            (Timeframe::D1, 14),
        ];

        for (tf, limit) in timeframes {
            match self.market.fetch_ohlcv(tf, limit).await {
                Ok(data) => {
                    self.data_cache.insert(tf, data);
                }
                Err(e) => {
                    debug!("Data refresh {}: {}", tf, e);
                }
            }
        }

        // 4H by resampling
        match self.market.get_4h(200).await {
            Ok(data) => {
                self.data_cache.insert(Timeframe::H4, data);
            }
            Err(e) => {
                debug!("Data refresh 4h: {}", e);
            }
        }
    }

    fn analyze_weekly(&mut self, cfg: &Config) {
        info!("--- Weekly Profile Analysis ---");
        let daily = match self.data_cache.get(&Timeframe::D1) {
            Some(d) => d,
            None => return,
        };
        let htf = match self.data_cache.get(&Timeframe::H1) {
            Some(d) => d,
            None => return,
        };

        let day = self.session.get_day_of_week();
        let bias = self
            .weekly_classifier
            .classify(daily, htf, &day, cfg);

        info!(
            "Profile: {} | Direction: {} | Confidence: {:.1}%",
            bias.profile,
            bias.direction,
            bias.confidence * 100.0
        );
        if bias.tgif_active {
            info!("TGIF ACTIVE");
        }

        self.weekly_bias = Some(bias);
    }

    fn log_alignment(&mut self, cfg: &Config) {
        if self.data_cache.is_empty() {
            return;
        }

        let summary = self
            .fractal
            .get_alignment_summary(&self.data_cache, cfg);

        info!("--- Alignment Dashboard ---");
        for (_, state) in &summary {
            let status = if state.aligned {
                "ALIGNED"
            } else {
                "NOT ALIGNED"
            };
            let details: Vec<String> = state
                .details
                .iter()
                .map(|d| format!("{}:{}", d.tf, d.trend))
                .collect();
            info!(
                "  {}: {} ({}) [{}]",
                state.name,
                status,
                state.direction,
                details.join(" | ")
            );
        }
    }

    async fn scan_scale(&mut self, scale_key: &str, cfg: &Config) {
        let weekly_bias = match &self.weekly_bias {
            Some(b) => b,
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
        if !self.session.should_trade_today(cfg, &profile_str) {
            return;
        }

        if self.scale_positions.contains_key(scale_key) {
            return;
        }

        // Cooldown after position close to prevent churning
        if let Some(&cooldown_until) = self.scale_cooldown.get(scale_key) {
            if Utc::now() < cooldown_until {
                return;
            }
            self.scale_cooldown.remove(&scale_key.to_string());
        }

        if !self.paper_trader.can_open_position(cfg) {
            return;
        }

        if self.data_cache.is_empty() {
            return;
        }

        if self.refiner.should_skip(scale_key, &self.session.current_session) {
            return;
        }

        let midnight_open = self.market.get_midnight_open().await.ok().flatten();

        // Evaluate this scale
        let scale = match self.fractal.scales.get_mut(scale_key) {
            Some(s) => s,
            None => return,
        };

        let signal = match scale.evaluate(&self.data_cache, midnight_open, &self.session, cfg) {
            Some(s) => s,
            None => return,
        };

        // Cross-scale confluence
        let all_signals =
            self.fractal
                .evaluate_all(&self.data_cache, midnight_open, &self.session, cfg);

        let signal = all_signals
            .into_iter()
            .find(|s| s.scale == scale_key)
            .unwrap_or(signal);

        let min_conf = cfg.hft_scales[scale_key].min_confidence;
        if signal.confidence < min_conf {
            return;
        }

        // Minimum TP distance filter: ensure expected profit > round-trip fees
        let tp_dist_pct = (signal.take_profit - signal.entry_price).abs() / signal.entry_price;
        let round_trip_fee = (cfg.fee_rate + cfg.slippage_rate) * 2.0;
        let min_tp_multiple: f64 = std::env::var("MIN_TP_MULTIPLE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(6.0);
        if tp_dist_pct < round_trip_fee * min_tp_multiple {
            debug!(
                "Skipping {} signal: TP dist {:.4}% < min {:.4}%",
                scale_key,
                tp_dist_pct * 100.0,
                round_trip_fee * min_tp_multiple * 100.0
            );
            return;
        }

        // Log the signal
        info!("{}", "=".repeat(60));
        info!("HFT SIGNAL — {}", signal.scale_name);
        info!("  Direction: {}", signal.direction);
        info!("  Entry: ${:.2}", signal.entry_price);
        info!("  Stop Loss: ${:.2} [{}]", signal.stop_loss, signal.stop_mode);
        info!("  {}", signal.stop_reason);
        info!("  Take Profit: ${:.2} [{}]", signal.take_profit, signal.tp_label);
        for lvl in &signal.tp_levels {
            let flag = if lvl.pda_confluence { " *PDA*" } else { "" };
            info!("    {}: ${:.2}{}", lvl.label, lvl.price, flag);
        }
        info!("  Confidence: {:.1}%", signal.confidence * 100.0);
        info!(
            "  CISD: {}",
            if signal.cisd_confirmed {
                "Confirmed"
            } else {
                "No"
            }
        );
        info!("  Cross-Scale: {} scale(s)", signal.cross_scale_confluence);
        if !signal.alignment.is_empty() {
            let align_str: Vec<String> = signal
                .alignment
                .iter()
                .map(|a| format!("{}:{}", a.tf, a.trend))
                .collect();
            info!("  Alignment: {}", align_str.join(" | "));
        }
        info!("  {}", signal.reason);

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
        if let Some(pos) = self.paper_trader.open_position(&trade_signal, scale_key, Some(metadata)) {
            let pos_id = pos.id;
            let size_usd = pos.size_usd;
            let size_btc = pos.size_btc;
            self.scale_positions.insert(scale_key.to_string(), pos_id);

            info!(
                "  Position #{} opened: ${:.2} ({:.6} BTC)",
                pos_id, size_usd, size_btc
            );

            if let Some(ref kr) = self.paper_trader.last_kelly_result {
                let default_str = if kr.using_default {
                    "default"
                } else {
                    "calculated"
                };
                info!(
                    "  Kelly: {:.4} ({}) | Edge: {:+.4} | Sample: {}",
                    kr.applied_fraction, default_str, kr.edge, kr.sample_size
                );
            }
        }
        info!("{}", "=".repeat(60));
    }

    async fn check_positions(&mut self, _cfg: &Config) {
        let open_pos: Vec<(usize, Direction, f64, String)> = self
            .paper_trader
            .positions
            .iter()
            .enumerate()
            .filter(|(_, p)| p.status == PositionStatus::Open)
            .map(|(i, p)| (i, p.direction, p.stop_loss, p.scale.clone()))
            .collect();

        if open_pos.is_empty() {
            return;
        }

        let current_price = match self.market.get_current_price().await {
            Ok(p) => p,
            Err(e) => {
                error!("Position check error: {}", e);
                return;
            }
        };

        // Trail stops using scale-matched timeframe
        let trail_tf_env = std::env::var("TRAIL_TF").unwrap_or_default();
        for &(_, direction, stop_loss, ref scale) in &open_pos {
            let trail_tf = if !trail_tf_env.is_empty() {
                match trail_tf_env.as_str() {
                    "1m" => Timeframe::M1,
                    "5m" => Timeframe::M5,
                    "15m" => Timeframe::M15,
                    _ => Timeframe::M5,
                }
            } else {
                match scale.as_str() {
                    "1m" => Timeframe::M1,
                    "5m" => Timeframe::M5,
                    "15m" => Timeframe::M15,
                    _ => Timeframe::M5,
                }
            };
            if let Some(trail_df) = self.data_cache.get(&trail_tf) {
                let mut trail_engine = StopLossEngine::new();
                if let Some(new_sl) =
                    trail_engine.get_trailing_stop(direction, stop_loss, trail_df, None)
                {
                    for pos in &mut self.paper_trader.positions {
                        if pos.status == PositionStatus::Open
                            && pos.direction == direction
                            && (pos.stop_loss - stop_loss).abs() < 0.01
                        {
                            let old_sl = pos.stop_loss;
                            pos.stop_loss = new_sl.price;
                            info!(
                                "Position #{} TRAIL: ${:.2} -> ${:.2}",
                                pos.id, old_sl, new_sl.price
                            );
                            break;
                        }
                    }
                }
            }
        }

        // Log partial exits
        for pos in &mut self.paper_trader.positions {
            for pe in &mut pos.partial_exits {
                if !pe.logged {
                    info!(
                        "Position #{} PARTIAL TP ({} SD): {:.6} BTC @ ${:.2} PnL ${:+.2}",
                        pos.id, pe.level, pe.size_btc, pe.price, pe.pnl
                    );
                    pe.logged = true;
                }
            }
        }

        let closed = self.paper_trader.check_positions(current_price);
        self.closed_since_analysis += closed.len();

        for pos in &closed {
            let result = if pos.pnl > 0.0 { "WIN" } else { "LOSS" };
            let partials = pos.partial_exits.len();
            let partial_info = if partials > 0 {
                format!(" ({} partials)", partials)
            } else {
                String::new()
            };
            info!(
                "Position #{} CLOSED ({}){}: PnL ${:+.2} | ${:.2} -> ${:.2}",
                pos.id,
                result,
                partial_info,
                pos.pnl,
                pos.entry_price,
                pos.exit_price.unwrap_or(0.0),
            );

            // Remove from scale_positions and set cooldown
            let keys_to_remove: Vec<String> = self
                .scale_positions
                .iter()
                .filter(|(_, &pid)| pid == pos.id)
                .map(|(k, _)| k.clone())
                .collect();
            let cooldown_mins: i64 = std::env::var("COOLDOWN_MINUTES")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(15);
            for key in keys_to_remove {
                self.scale_positions.remove(&key);
                self.scale_cooldown.insert(
                    key,
                    Utc::now() + chrono::Duration::minutes(cooldown_mins),
                );
            }
        }
    }

    async fn run_analysis(&mut self) {
        let records: Vec<_> = self.paper_trader.trade_records.values().cloned().collect();
        let closed: Vec<_> = records
            .iter()
            .filter(|r| r.outcome == "win" || r.outcome == "loss")
            .cloned()
            .collect();

        if closed.len() < self.refiner.min_sample {
            return;
        }

        let mut cfg = self.config.write().await;
        let adjustments = self.refiner.refine(&closed, &mut cfg);

        if !adjustments.is_empty() {
            info!("--- Strategy Refinement ---");
            for adj in &adjustments {
                if adj.parameter.starts_with("WARNING:") {
                    warn!("  {}", adj.reason);
                } else {
                    info!(
                        "  {}: {:.4} -> {:.4} ({})",
                        adj.parameter, adj.old_value, adj.new_value, adj.reason
                    );
                }
            }
            if !self.refiner.skip_combos.is_empty() {
                info!("  Skip combos: {:?}", self.refiner.skip_combos);
            }
        } else {
            debug!("Analysis complete — no adjustments needed");
        }
    }

    async fn print_status(&mut self) {
        let cfg = self.config.read().await;
        let stats = self.paper_trader.get_stats();
        self.session.update(&cfg, None);

        info!(
            "Session: {} (weight: {})",
            self.session.current_session, self.session.session_weight
        );
        info!("Day: {}", self.session.get_day_of_week());
        info!("Balance: ${:.2}", stats.balance);
        info!(
            "Trades: {} | Win Rate: {}%",
            stats.total_trades, stats.win_rate
        );
        info!("PnL: ${:+.2}", stats.total_pnl);
        info!(
            "Open: {} | Scale slots: {:?}",
            stats.open_positions, self.scale_positions
        );

        let default_str = if stats.kelly_using_default {
            "default"
        } else {
            "calculated"
        };
        info!(
            "Kelly: f={:.4} ({}) | Edge: {:+.4} | Sample: {}",
            stats.kelly_fraction, default_str, stats.kelly_edge, stats.kelly_sample
        );

        let scale_kelly = self.paper_trader.get_kelly_by_scale();
        for (s, kr) in &scale_kelly {
            if kr.sample_size > 0 {
                info!(
                    "  Kelly {}: f={:.4} WR={:.1}% Payoff={:.2} Edge={:+.4} ({} trades)",
                    s,
                    kr.applied_fraction,
                    kr.win_rate * 100.0,
                    kr.payoff_ratio,
                    kr.edge,
                    kr.sample_size
                );
            }
        }
    }

    async fn shutdown(&mut self) {
        info!("Shutting down...");
        self.print_status().await;
        info!("Bot stopped.");
    }
}
