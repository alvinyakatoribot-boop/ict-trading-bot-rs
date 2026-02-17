use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::core::pd_arrays::Pda;
use crate::models::{CandleSeries, Trend};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CisdConfirmation {
    pub direction: Trend,
    pub breaker: Pda,
    pub confirmation_candle: DateTime<Utc>,
    pub close_price: f64,
    pub strength: f64,
}

pub struct CisdDetector {
    pub confirmed_cisds: Vec<CisdConfirmation>,
}

impl CisdDetector {
    pub fn new() -> Self {
        Self {
            confirmed_cisds: Vec::new(),
        }
    }

    pub fn check(
        &mut self,
        candles: &CandleSeries,
        breaker_blocks: &[Pda],
    ) -> &[CisdConfirmation] {
        self.confirmed_cisds.clear();

        if breaker_blocks.is_empty() || candles.is_empty() {
            return &self.confirmed_cisds;
        }

        let latest_candles = candles.tail(5);

        for brk in breaker_blocks {
            for i in 0..latest_candles.len() {
                let candle = &latest_candles[i];

                match brk.direction {
                    Trend::Bullish => {
                        // Bullish CISD: candle body closes above breaker high
                        if candle.close > brk.high && candle.close > candle.open {
                            self.confirmed_cisds.push(CisdConfirmation {
                                direction: Trend::Bullish,
                                breaker: brk.clone(),
                                confirmation_candle: candle.timestamp,
                                close_price: candle.close,
                                strength: ((candle.close - brk.high)
                                    / (brk.high - brk.low + 0.01))
                                    .min(1.0),
                            });
                            break;
                        }
                    }
                    Trend::Bearish => {
                        // Bearish CISD: candle body closes below breaker low
                        if candle.close < brk.low && candle.close < candle.open {
                            self.confirmed_cisds.push(CisdConfirmation {
                                direction: Trend::Bearish,
                                breaker: brk.clone(),
                                confirmation_candle: candle.timestamp,
                                close_price: candle.close,
                                strength: ((brk.low - candle.close)
                                    / (brk.high - brk.low + 0.01))
                                    .min(1.0),
                            });
                            break;
                        }
                    }
                    Trend::Neutral => {}
                }
            }
        }

        &self.confirmed_cisds
    }

    pub fn has_bullish_cisd(&self) -> bool {
        self.confirmed_cisds
            .iter()
            .any(|c| c.direction == Trend::Bullish)
    }

    pub fn has_bearish_cisd(&self) -> bool {
        self.confirmed_cisds
            .iter()
            .any(|c| c.direction == Trend::Bearish)
    }

    pub fn strongest(&self) -> Option<&CisdConfirmation> {
        self.confirmed_cisds
            .iter()
            .max_by(|a, b| a.strength.partial_cmp(&b.strength).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{PdaType, Timeframe, Zone};
    use crate::test_helpers::make_candles;

    fn make_bullish_breaker() -> Pda {
        Pda {
            pda_type: PdaType::BRK,
            direction: Trend::Bullish,
            zone: Zone::Discount,
            high: 105.0,
            low: 100.0,
            midpoint: 102.5,
            timestamp: chrono::Utc::now(),
            timeframe: Timeframe::M1,
            strength: 0.7,
        }
    }

    fn make_bearish_breaker() -> Pda {
        Pda {
            pda_type: PdaType::BRK,
            direction: Trend::Bearish,
            zone: Zone::Premium,
            high: 110.0,
            low: 105.0,
            midpoint: 107.5,
            timestamp: chrono::Utc::now(),
            timeframe: Timeframe::M1,
            strength: 0.7,
        }
    }

    #[test]
    fn bullish_cisd_confirmed() {
        // Candle closes above breaker high with bullish body
        let candles = make_candles(&[
            (100.0, 102.0, 99.0, 101.0),
            (101.0, 103.0, 100.0, 102.0),
            (102.0, 108.0, 101.0, 107.0),  // closes above 105 with bullish body
        ]);
        let breakers = vec![make_bullish_breaker()];
        let mut det = CisdDetector::new();
        det.check(&candles, &breakers);
        assert!(det.has_bullish_cisd());
    }

    #[test]
    fn bearish_cisd_confirmed() {
        // Candle closes below breaker low with bearish body
        let candles = make_candles(&[
            (110.0, 112.0, 108.0, 109.0),
            (109.0, 110.0, 107.0, 108.0),
            (108.0, 109.0, 102.0, 103.0),  // closes below 105 with bearish body
        ]);
        let breakers = vec![make_bearish_breaker()];
        let mut det = CisdDetector::new();
        det.check(&candles, &breakers);
        assert!(det.has_bearish_cisd());
    }

    #[test]
    fn no_cisd_when_price_inside_breaker() {
        // Candle stays within breaker range
        let candles = make_candles(&[
            (101.0, 104.0, 100.0, 103.0),
            (103.0, 104.5, 102.0, 104.0),
        ]);
        let breakers = vec![make_bullish_breaker()]; // high=105
        let mut det = CisdDetector::new();
        det.check(&candles, &breakers);
        assert!(!det.has_bullish_cisd());
    }

    #[test]
    fn no_cisd_with_empty_breakers() {
        let candles = make_candles(&[(100.0, 110.0, 95.0, 108.0)]);
        let mut det = CisdDetector::new();
        det.check(&candles, &[]);
        assert!(!det.has_bullish_cisd());
        assert!(!det.has_bearish_cisd());
    }

    #[test]
    fn strongest_returns_highest_strength() {
        let candles = make_candles(&[
            (100.0, 102.0, 99.0, 101.0),
            (101.0, 108.0, 100.0, 107.0),
            (107.0, 120.0, 106.0, 118.0),
        ]);
        let brk1 = make_bullish_breaker(); // high=105
        let mut brk2 = make_bullish_breaker();
        brk2.high = 103.0;
        brk2.low = 98.0;
        let breakers = vec![brk1, brk2];
        let mut det = CisdDetector::new();
        det.check(&candles, &breakers);
        let strongest = det.strongest();
        assert!(strongest.is_some());
    }
}
