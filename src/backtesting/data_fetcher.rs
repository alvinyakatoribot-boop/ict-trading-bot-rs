use anyhow::Result;
use chrono::{DateTime, Utc};
use std::path::Path;
use std::time::Duration;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::exchange::CoinbaseClient;
use crate::models::{Candle, CandleSeries, Timeframe};

const MAX_CANDLES_PER_REQUEST: u64 = 300;
const RATE_LIMIT_SLEEP_MS: u64 = 250;

/// Fetch historical data from Coinbase and save to local JSON files.
pub async fn fetch_and_cache(
    cfg: &Config,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    data_dir: &str,
    timeframes: &[Timeframe],
) -> Result<Vec<(Timeframe, Vec<Candle>)>> {
    std::fs::create_dir_all(data_dir)?;

    let mut client = CoinbaseClient::new(cfg);
    let mut results = Vec::new();

    for &tf in timeframes {
        // Round to day boundaries for cache reuse across runs
        let cache_file = format!(
            "{}/{}_{}_{}_to_{}.json",
            data_dir,
            cfg.symbol,
            tf,
            start.format("%Y%m%d"),
            end.format("%Y%m%d")
        );

        // Try loading from cache first
        if Path::new(&cache_file).exists() {
            info!("Loading cached {} data from {}", tf, cache_file);
            let content = std::fs::read_to_string(&cache_file)?;
            let candles: Vec<Candle> = serde_json::from_str(&content)?;
            info!("  Loaded {} candles", candles.len());
            results.push((tf, candles));
            continue;
        }

        // Skip 4H — we'll resample from H1
        if tf == Timeframe::H4 {
            info!("  Skipping 4H (will resample from 1H)");
            results.push((tf, Vec::new()));
            continue;
        }

        info!(
            "Fetching {} data from Coinbase ({} to {})...",
            tf,
            start.format("%Y-%m-%d"),
            end.format("%Y-%m-%d")
        );

        let candles = fetch_range(&mut client, tf, start, end).await?;
        info!("  Fetched {} {} candles total", candles.len(), tf);

        // Save to cache
        let json = serde_json::to_string(&candles)?;
        std::fs::write(&cache_file, json)?;
        info!("  Cached to {}", cache_file);

        results.push((tf, candles));
    }

    // Generate 4H from H1 if needed
    if timeframes.contains(&Timeframe::H4) {
        let h1_candles = results
            .iter()
            .find(|(tf, _)| *tf == Timeframe::H1)
            .map(|(_, c)| c.clone())
            .unwrap_or_default();

        if !h1_candles.is_empty() {
            let h1_series = CandleSeries::new(h1_candles);
            let h4_series = h1_series.resample(Duration::from_secs(14400));
            let h4_candles: Vec<Candle> = h4_series.into_iter().collect();
            info!("Generated {} 4H candles from H1 data", h4_candles.len());

            if let Some(entry) = results.iter_mut().find(|(tf, _)| *tf == Timeframe::H4) {
                entry.1 = h4_candles;
            }
        }
    }

    Ok(results)
}

/// Fetch a date range by paginating through the API in chunks.
async fn fetch_range(
    client: &mut CoinbaseClient,
    tf: Timeframe,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Vec<Candle>> {
    let mut all_candles: Vec<Candle> = Vec::new();
    let tf_secs = tf.as_seconds();
    let chunk_duration = tf_secs * MAX_CANDLES_PER_REQUEST;

    let start_ts = start.timestamp() as u64;
    let end_ts = end.timestamp() as u64;
    let mut chunk_start = start_ts;

    let total_chunks = ((end_ts - start_ts) as f64 / chunk_duration as f64).ceil() as usize;
    let mut chunk_num = 0;

    while chunk_start < end_ts {
        let chunk_end = (chunk_start + chunk_duration).min(end_ts);
        chunk_num += 1;

        if chunk_num % 10 == 0 || chunk_num == 1 {
            debug!(
                "  {} chunk {}/{} ({} candles so far)",
                tf, chunk_num, total_chunks, all_candles.len()
            );
        }

        match client
            .fetch_ohlcv_range(tf, chunk_start, chunk_end)
            .await
        {
            Ok(series) => {
                for candle in series {
                    all_candles.push(candle);
                }
            }
            Err(e) => {
                warn!("  Error fetching {} chunk {}: {}", tf, chunk_num, e);
                // On rate limit or error, back off
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }

        chunk_start = chunk_end;

        // Rate limiting between chunks
        tokio::time::sleep(Duration::from_millis(RATE_LIMIT_SLEEP_MS)).await;
    }

    // Deduplicate by timestamp and sort
    all_candles.sort_by_key(|c| c.timestamp);
    all_candles.dedup_by_key(|c| c.timestamp);

    Ok(all_candles)
}
