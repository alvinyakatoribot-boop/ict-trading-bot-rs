mod common;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;

use ict_trading_bot::config::Config;
use ict_trading_bot::core::sessions::SessionManager;
use ict_trading_bot::exchange::Exchange;
use ict_trading_bot::models::{Candle, CandleSeries, Direction, PositionStatus, Timeframe};
use ict_trading_bot::strategies::fractal_engine::FractalEngine;
use ict_trading_bot::strategies::weekly_profiles::WeeklyProfileClassifier;
use ict_trading_bot::trading::paper_trader::PaperTrader;




/// A mock exchange that returns canned bullish BTC data.
struct MockExchange {
    data: HashMap<Timeframe, CandleSeries>,
    current_price: f64,
}

impl MockExchange {
    fn new() -> Self {
        let base = DateTime::parse_from_rfc3339("2024-01-17T07:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        // Build bullish data across all timeframes — all trending up
        let m1 = Self::make_tf_data(base, 200, Duration::minutes(1), 40000.0, 2.0);
        let m5 = Self::make_tf_data(base, 200, Duration::minutes(5), 40000.0, 10.0);
        let m15 = Self::make_tf_data(base, 200, Duration::minutes(15), 40000.0, 30.0);
        let h1 = Self::make_tf_data(base, 200, Duration::hours(1), 39000.0, 50.0);
        let h4 = Self::make_tf_data(base, 200, Duration::hours(4), 38000.0, 100.0);
        let d1 = Self::make_tf_data(base, 14, Duration::days(1), 37000.0, 500.0);

        let current = m1.last().unwrap().close;

        let mut data = HashMap::new();
        data.insert(Timeframe::M1, m1);
        data.insert(Timeframe::M5, m5);
        data.insert(Timeframe::M15, m15);
        data.insert(Timeframe::H1, h1);
        data.insert(Timeframe::H4, h4);
        data.insert(Timeframe::D1, d1);

        Self {
            data,
            current_price: current,
        }
    }

    fn make_tf_data(
        base: DateTime<Utc>,
        count: usize,
        interval: Duration,
        start_price: f64,
        step: f64,
    ) -> CandleSeries {
        // Build a staircase pattern with swings for structure detection
        let candles: Vec<Candle> = (0..count)
            .map(|i| {
                let wave = i / 14; // every 14 candles = one wave
                let pos_in_wave = i % 14;

                let wave_base = start_price + wave as f64 * step * 8.0;

                // Up for 8 candles, down for 6 (net up)
                let price = if pos_in_wave < 8 {
                    wave_base + pos_in_wave as f64 * step
                } else {
                    let peak = wave_base + 8.0 * step;
                    peak - (pos_in_wave - 8) as f64 * step * 0.5
                };

                Candle {
                    timestamp: base + interval * i as i32,
                    open: price,
                    high: price + step * 0.5,
                    low: price - step * 0.3,
                    close: price + step * 0.2,
                    volume: 100.0,
                }
            })
            .collect();

        CandleSeries::new(candles)
    }
}

#[async_trait]
impl Exchange for MockExchange {
    async fn fetch_ohlcv(&mut self, tf: Timeframe, _limit: usize) -> Result<CandleSeries> {
        Ok(self.data.get(&tf).cloned().unwrap_or_default())
    }

    async fn get_current_price(&mut self) -> Result<f64> {
        Ok(self.current_price)
    }

    async fn get_4h(&mut self, _limit: usize) -> Result<CandleSeries> {
        Ok(self
            .data
            .get(&Timeframe::H4)
            .cloned()
            .unwrap_or_default())
    }

    async fn get_midnight_open(&mut self) -> Result<Option<f64>> {
        Ok(Some(40000.0))
    }
}

fn test_config() -> Config {
    let mut cfg = Config::from_env();
    cfg.paper_trade = true;
    cfg.initial_balance = 200.0;
    cfg.coinbase_api_key = String::new();
    cfg.coinbase_api_secret = String::new();
    cfg.log_dir = std::env::temp_dir()
        .join(format!("ict_bot_integ_{}", std::process::id()))
        .to_string_lossy()
        .to_string();
    cfg
}

#[tokio::test]
async fn full_pipeline_without_exchange() {
    let cfg = test_config();

    // 1. Build MockExchange and gather data
    let mock = MockExchange::new();
    let data_cache = mock.data.clone();
    let current_price = mock.current_price;

    // 2. Create SessionManager at a killzone time
    // Use a UTC time that maps to ~8am ET (ny_forex killzone) in January (EST = UTC-5)
    // 8am ET = 13:00 UTC
    let session_time = DateTime::parse_from_rfc3339("2024-01-17T13:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let mut session = SessionManager::new(&cfg);
    session.update(&cfg, Some(session_time));
    assert!(
        session.is_killzone(),
        "Expected killzone session, got: {}",
        session.current_session
    );

    // 3. Run WeeklyProfileClassifier
    let daily = data_cache.get(&Timeframe::D1).unwrap();
    let htf = data_cache.get(&Timeframe::H1).unwrap();
    let mut wpc = WeeklyProfileClassifier::new();
    let day_name = "Wednesday"; // 2024-01-17 is a Wednesday
    let bias = wpc.classify(daily, htf, day_name, &cfg);
    // With enough daily data, should not be Undetermined
    assert!(
        bias.confidence > 0.0,
        "Expected some confidence, got: {:?}",
        bias
    );

    // 4. Run FractalEngine::evaluate_all
    let mut fractal = FractalEngine::new(&cfg);
    let _signals = fractal.evaluate_all(
        &data_cache,
        Some(40000.0), // midnight open reference
        &session,
        &cfg,
    );
    // Note: signals may or may not be generated depending on market conditions
    // This test validates the pipeline runs without panics

    // 5. Open a position via PaperTrader (manually, to test the trader)
    let mut trader = PaperTrader::new(&cfg);
    let initial_balance = trader.balance;

    let signal = ict_trading_bot::strategies::signals::TradeSignal {
        direction: Direction::Long,
        entry_price: current_price,
        stop_loss: current_price - 500.0,
        take_profit: current_price + 1000.0,
        pda_engaged: None,
        cisd_confirmed: false,
        confidence: 0.7,
        session: session.current_session.clone(),
        session_weight: session.session_weight,
        reason: "Integration test signal".to_string(),
        tp_levels: None,
    };

    let pos = trader.open_position(&signal, "5m", None);
    assert!(pos.is_some(), "Should be able to open position");
    let pos = pos.unwrap();
    assert_eq!(pos.status, PositionStatus::Open);
    let _entry = pos.entry_price;

    // 6. Simulate TP hit
    let tp_price = current_price + 1100.0; // above take_profit
    let closed = trader.check_positions(tp_price);
    assert_eq!(closed.len(), 1, "Position should be closed at TP");
    assert_eq!(closed[0].status, PositionStatus::ClosedTp);

    // 7. Assert: balance increased, position closed
    assert!(
        trader.balance > initial_balance,
        "Balance should increase after TP hit: {} vs {}",
        trader.balance,
        initial_balance
    );
    assert!(
        closed[0].pnl > 0.0,
        "PnL should be positive after TP hit"
    );

    // Verify the pipeline components all ran without panics
    // The fractal engine alignment check exercised: MarketStructure, PdArrayDetector,
    // CisdDetector, StdDevProjector, StopLossEngine across multiple timeframes
}
