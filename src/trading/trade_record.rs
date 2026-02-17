use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeMetadata {
    pub scale: String,
    pub direction: String,
    pub confidence: f64,
    pub session: String,
    pub session_weight: f64,
    pub cisd_confirmed: bool,
    #[serde(default)]
    pub pda_type: String,
    #[serde(default)]
    pub pda_direction: String,
    #[serde(default)]
    pub pda_zone: String,
    #[serde(default)]
    pub pda_strength: f64,
    #[serde(default)]
    pub stop_mode: String,
    #[serde(default)]
    pub tp_label: String,
    #[serde(default)]
    pub tp_levels: Vec<TpLevelInfo>,
    #[serde(default = "default_one")]
    pub cross_scale_confluence: usize,
    #[serde(default)]
    pub alignment: Vec<AlignmentInfo>,
    #[serde(default)]
    pub weekly_profile: String,
    #[serde(default)]
    pub weekly_direction: String,
    #[serde(default)]
    pub weekly_confidence: f64,
    #[serde(default)]
    pub day_of_week: String,
    #[serde(default)]
    pub kelly_fraction: f64,
}

fn default_one() -> usize {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TpLevelInfo {
    pub label: String,
    pub price: f64,
    #[serde(default)]
    pub pda_confluence: bool,
    #[serde(default)]
    pub level: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlignmentInfo {
    pub tf: String,
    pub trend: String,
    #[serde(default)]
    pub bos: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeRecord {
    pub position_id: u64,
    pub metadata: TradeMetadata,
    #[serde(default)]
    pub outcome: String,
    #[serde(default)]
    pub pnl: f64,
    #[serde(default)]
    pub hold_duration_seconds: f64,
}
