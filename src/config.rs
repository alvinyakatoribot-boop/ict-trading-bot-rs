use crate::models::Timeframe;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub type SharedConfig = Arc<RwLock<Config>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTime {
    pub start: (u32, u32),
    pub end: (u32, u32),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HftScaleConfig {
    pub name: String,
    pub entry_tf: Timeframe,
    pub alignment_tfs: Vec<Timeframe>,
    pub structure_tf: Timeframe,
    pub confirm_tf: Timeframe,
    pub scan_interval: u64,
    pub min_confidence: f64,
    pub weight: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayRatings {
    pub monday: f64,
    pub tuesday: f64,
    pub wednesday: f64,
    pub thursday: f64,
    pub friday: f64,
    pub saturday: f64,
    pub sunday: f64,
}

impl DayRatings {
    pub fn get(&self, day: &str) -> f64 {
        match day {
            "Monday" => self.monday,
            "Tuesday" => self.tuesday,
            "Wednesday" => self.wednesday,
            "Thursday" => self.thursday,
            "Friday" => self.friday,
            "Saturday" => self.saturday,
            "Sunday" => self.sunday,
            _ => 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    // Exchange
    pub exchange: String,
    pub symbol: String,
    pub coinbase_api_key: String,
    pub coinbase_api_secret: String,

    // Paper Trading
    pub paper_trade: bool,
    pub initial_balance: f64,

    // Risk
    pub max_daily_loss: f64,
    pub max_open_positions: usize,

    // Fees & Slippage (as fraction, e.g., 0.001 = 0.1%)
    pub fee_rate: f64,
    pub slippage_rate: f64,

    // Sessions (stored as minute offsets from midnight ET)
    pub sessions: HashMap<String, SessionTime>,
    pub session_weights: HashMap<String, f64>,

    // HFT Scales
    pub hft_scales: HashMap<String, HftScaleConfig>,

    // Cross-scale confluence
    pub cross_scale_confluence_bonus: f64,

    // Weekly Profile Day Ratings
    pub day_ratings: HashMap<String, DayRatings>,
    pub min_day_rating: f64,

    // PD Array Settings
    pub fvg_min_gap_percent: f64,
    pub ob_lookback: usize,
    pub breaker_lookback: usize,

    // TGIF
    pub tgif_retrace_min: f64,
    pub tgif_retrace_max: f64,

    // Self-Learning
    pub analysis_interval: u64,
    pub min_sample_per_bucket: usize,
    pub adjustment_step: f64,

    // Logging
    pub log_dir: String,
    pub log_level: String,
}

impl Config {
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();

        let env = |key: &str, default: &str| -> String {
            std::env::var(key).unwrap_or_else(|_| default.to_string())
        };

        let mut sessions = HashMap::new();
        sessions.insert(
            "asian".to_string(),
            SessionTime {
                start: (20, 0),
                end: (0, 0),
            },
        );
        sessions.insert(
            "london".to_string(),
            SessionTime {
                start: (2, 0),
                end: (5, 0),
            },
        );
        sessions.insert(
            "ny_forex".to_string(),
            SessionTime {
                start: (7, 0),
                end: (10, 0),
            },
        );
        sessions.insert(
            "ny_indices".to_string(),
            SessionTime {
                start: (8, 30),
                end: (12, 0),
            },
        );

        let mut session_weights = HashMap::new();
        session_weights.insert("london".to_string(), 1.5);
        session_weights.insert("ny_forex".to_string(), 1.5);
        session_weights.insert("ny_indices".to_string(), 1.3);
        session_weights.insert("asian".to_string(), 0.3);
        session_weights.insert("off_session".to_string(), 0.3);

        let mut hft_scales = HashMap::new();
        hft_scales.insert(
            "1m".to_string(),
            HftScaleConfig {
                name: "1m Scalp".to_string(),
                entry_tf: Timeframe::M1,
                alignment_tfs: vec![Timeframe::M5, Timeframe::M15, Timeframe::H1],
                structure_tf: Timeframe::M5,
                confirm_tf: Timeframe::M1,
                scan_interval: 10,
                min_confidence: 0.7,
                weight: 1.0,
            },
        );
        hft_scales.insert(
            "5m".to_string(),
            HftScaleConfig {
                name: "5m Intraday".to_string(),
                entry_tf: Timeframe::M5,
                alignment_tfs: vec![Timeframe::M15, Timeframe::H1, Timeframe::H4],
                structure_tf: Timeframe::M15,
                confirm_tf: Timeframe::M5,
                scan_interval: 30,
                min_confidence: 0.55,
                weight: 1.0,
            },
        );
        hft_scales.insert(
            "15m".to_string(),
            HftScaleConfig {
                name: "15m Swing".to_string(),
                entry_tf: Timeframe::M15,
                alignment_tfs: vec![Timeframe::H1, Timeframe::H4, Timeframe::D1],
                structure_tf: Timeframe::H1,
                confirm_tf: Timeframe::M15,
                scan_interval: 60,
                min_confidence: 0.7,
                weight: 1.0,
            },
        );

        let mut day_ratings = HashMap::new();
        day_ratings.insert(
            "classic_expansion".to_string(),
            DayRatings {
                monday: 0.0,
                tuesday: 4.0,
                wednesday: 5.0,
                thursday: 4.5,
                friday: 3.5,
                saturday: 3.0,
                sunday: 3.0,
            },
        );
        day_ratings.insert(
            "midweek_reversal".to_string(),
            DayRatings {
                monday: 0.0,
                tuesday: 3.0,
                wednesday: 3.5,
                thursday: 5.0,
                friday: 4.5,
                saturday: 3.0,
                sunday: 3.0,
            },
        );
        day_ratings.insert(
            "consolidation_reversal".to_string(),
            DayRatings {
                monday: 0.0,
                tuesday: 2.0,
                wednesday: 2.5,
                thursday: 4.0,
                friday: 5.0,
                saturday: 3.0,
                sunday: 3.0,
            },
        );
        day_ratings.insert(
            "undetermined".to_string(),
            DayRatings {
                monday: 0.0,
                tuesday: 3.0,
                wednesday: 3.5,
                thursday: 3.5,
                friday: 3.0,
                saturday: 3.0,
                sunday: 3.0,
            },
        );

        Config {
            exchange: "coinbase".to_string(),
            symbol: "BTC-USD".to_string(),
            coinbase_api_key: env("COINBASE_API_KEY", ""),
            coinbase_api_secret: env("COINBASE_API_SECRET", "").replace("\\n", "\n"),
            paper_trade: env("PAPER_TRADE", "true").to_lowercase() == "true",
            initial_balance: env("INITIAL_BALANCE", "200")
                .parse()
                .unwrap_or(200.0),
            max_daily_loss: 0.03,
            max_open_positions: 3,
            fee_rate: env("FEE_RATE", "0.001").parse().unwrap_or(0.001),         // 0.1% per trade
            slippage_rate: env("SLIPPAGE_RATE", "0.0005").parse().unwrap_or(0.0005), // 0.05% per trade
            sessions,
            session_weights,
            hft_scales,
            cross_scale_confluence_bonus: 0.1,
            day_ratings,
            min_day_rating: 3.0,
            fvg_min_gap_percent: env("FVG_MIN_GAP", "0.0005").parse().unwrap_or(0.0005),
            ob_lookback: env("OB_LOOKBACK", "20").parse().unwrap_or(20),
            breaker_lookback: env("BREAKER_LOOKBACK", "30").parse().unwrap_or(30),
            tgif_retrace_min: 0.20,
            tgif_retrace_max: 0.30,
            analysis_interval: 3600,
            min_sample_per_bucket: 10,
            adjustment_step: 0.02,
            log_dir: "logs".to_string(),
            log_level: "INFO".to_string(),
        }
    }

    pub fn shared(self) -> SharedConfig {
        Arc::new(RwLock::new(self))
    }
}
