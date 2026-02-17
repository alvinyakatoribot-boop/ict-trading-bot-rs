use serde::{Deserialize, Serialize};

use crate::core::pd_arrays::Pda;
use crate::models::Direction;
use crate::trading::trade_record::TpLevelInfo;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeSignal {
    pub direction: Direction,
    pub entry_price: f64,
    pub stop_loss: f64,
    pub take_profit: f64,
    pub pda_engaged: Option<Pda>,
    pub cisd_confirmed: bool,
    pub confidence: f64,
    pub session: String,
    pub session_weight: f64,
    pub reason: String,
    #[serde(default)]
    pub tp_levels: Option<Vec<TpLevelInfo>>,
}
