use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Long,
    Short,
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Direction::Long => write!(f, "long"),
            Direction::Short => write!(f, "short"),
        }
    }
}

impl Direction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Direction::Long => "long",
            Direction::Short => "short",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trend {
    Bullish,
    Bearish,
    Neutral,
}

impl fmt::Display for Trend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Trend::Bullish => write!(f, "bullish"),
            Trend::Bearish => write!(f, "bearish"),
            Trend::Neutral => write!(f, "neutral"),
        }
    }
}

impl Trend {
    pub fn to_direction(self) -> Option<Direction> {
        match self {
            Trend::Bullish => Some(Direction::Long),
            Trend::Bearish => Some(Direction::Short),
            Trend::Neutral => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SwingType {
    High,
    Low,
}

impl fmt::Display for SwingType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SwingType::High => write!(f, "high"),
            SwingType::Low => write!(f, "low"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PdaType {
    OB,
    FVG,
    BRK,
    RB,
}

impl fmt::Display for PdaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PdaType::OB => write!(f, "OB"),
            PdaType::FVG => write!(f, "FVG"),
            PdaType::BRK => write!(f, "BRK"),
            PdaType::RB => write!(f, "RB"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Zone {
    Premium,
    Discount,
}

impl fmt::Display for Zone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Zone::Premium => write!(f, "premium"),
            Zone::Discount => write!(f, "discount"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StopMode {
    Wick,
    Body,
    Continuation,
}

impl fmt::Display for StopMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StopMode::Wick => write!(f, "wick"),
            StopMode::Body => write!(f, "body"),
            StopMode::Continuation => write!(f, "continuation"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionStatus {
    Open,
    ClosedTp,
    ClosedSl,
    ClosedManual,
}

impl fmt::Display for PositionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PositionStatus::Open => write!(f, "open"),
            PositionStatus::ClosedTp => write!(f, "closed_tp"),
            PositionStatus::ClosedSl => write!(f, "closed_sl"),
            PositionStatus::ClosedManual => write!(f, "closed_manual"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BosType {
    #[serde(rename = "bullish_bos")]
    BullishBos,
    #[serde(rename = "bearish_bos")]
    BearishBos,
}

impl fmt::Display for BosType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BosType::BullishBos => write!(f, "bullish_bos"),
            BosType::BearishBos => write!(f, "bearish_bos"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeeklyProfile {
    ClassicExpansion,
    MidweekReversal,
    ConsolidationReversal,
    Undetermined,
}

impl fmt::Display for WeeklyProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WeeklyProfile::ClassicExpansion => write!(f, "classic_expansion"),
            WeeklyProfile::MidweekReversal => write!(f, "midweek_reversal"),
            WeeklyProfile::ConsolidationReversal => write!(f, "consolidation_reversal"),
            WeeklyProfile::Undetermined => write!(f, "undetermined"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DrawOnLiquidity {
    #[serde(rename = "BSL")]
    Bsl,
    #[serde(rename = "SSL")]
    Ssl,
    #[serde(rename = "none")]
    None_,
}

impl fmt::Display for DrawOnLiquidity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DrawOnLiquidity::Bsl => write!(f, "BSL"),
            DrawOnLiquidity::Ssl => write!(f, "SSL"),
            DrawOnLiquidity::None_ => write!(f, "none"),
        }
    }
}
