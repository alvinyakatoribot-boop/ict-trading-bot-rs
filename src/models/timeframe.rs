use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Timeframe {
    #[serde(rename = "1m")]
    M1,
    #[serde(rename = "5m")]
    M5,
    #[serde(rename = "15m")]
    M15,
    #[serde(rename = "1h")]
    H1,
    #[serde(rename = "4h")]
    H4,
    #[serde(rename = "1d")]
    D1,
}

impl Timeframe {
    pub fn as_str(&self) -> &'static str {
        match self {
            Timeframe::M1 => "1m",
            Timeframe::M5 => "5m",
            Timeframe::M15 => "15m",
            Timeframe::H1 => "1h",
            Timeframe::H4 => "4h",
            Timeframe::D1 => "1d",
        }
    }

    pub fn as_duration(&self) -> Duration {
        match self {
            Timeframe::M1 => Duration::from_secs(60),
            Timeframe::M5 => Duration::from_secs(300),
            Timeframe::M15 => Duration::from_secs(900),
            Timeframe::H1 => Duration::from_secs(3600),
            Timeframe::H4 => Duration::from_secs(14400),
            Timeframe::D1 => Duration::from_secs(86400),
        }
    }

    /// Coinbase REST API granularity string
    pub fn coinbase_granularity(&self) -> &'static str {
        match self {
            Timeframe::M1 => "ONE_MINUTE",
            Timeframe::M5 => "FIVE_MINUTE",
            Timeframe::M15 => "FIFTEEN_MINUTE",
            Timeframe::H1 => "ONE_HOUR",
            Timeframe::H4 => "ONE_HOUR", // resample from 1h
            Timeframe::D1 => "ONE_DAY",
        }
    }

    pub fn from_str_loose(s: &str) -> Option<Timeframe> {
        match s {
            "1m" => Some(Timeframe::M1),
            "5m" => Some(Timeframe::M5),
            "15m" => Some(Timeframe::M15),
            "1h" => Some(Timeframe::H1),
            "4h" => Some(Timeframe::H4),
            "1d" => Some(Timeframe::D1),
            _ => None,
        }
    }

    pub fn as_seconds(&self) -> u64 {
        self.as_duration().as_secs()
    }
}

impl fmt::Display for Timeframe {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
