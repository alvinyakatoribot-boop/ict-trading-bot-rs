use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Timelike, Utc};
use chrono_tz::US::Eastern;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::exchange::Exchange;
use crate::models::{Candle, CandleSeries, Timeframe};

const BASE_URL: &str = "https://api.coinbase.com";
const MIN_REQUEST_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Serialize)]
struct JwtClaims {
    sub: String,
    iss: String,
    nbf: u64,
    exp: u64,
    uri: String,
}

#[derive(Debug, Deserialize)]
struct CandleResponse {
    candles: Vec<RawCandle>,
}

#[derive(Debug, Deserialize)]
struct RawCandle {
    start: String,
    low: String,
    high: String,
    open: String,
    close: String,
    volume: String,
}

#[derive(Debug, Deserialize)]
struct TickerResponse {
    trades: Vec<TickerTrade>,
}

#[derive(Debug, Deserialize)]
struct TickerTrade {
    price: String,
}

pub struct CoinbaseClient {
    client: Client,
    api_key: String,
    api_secret: String,
    symbol: String,
    last_request: Option<Instant>,
    cache: HashMap<String, (Instant, CandleSeries)>,
    cache_ttl: Duration,
}

impl CoinbaseClient {
    pub fn new(cfg: &Config) -> Self {
        Self {
            client: Client::new(),
            api_key: cfg.coinbase_api_key.clone(),
            api_secret: cfg.coinbase_api_secret.clone(),
            symbol: cfg.symbol.clone(),
            last_request: None,
            cache: HashMap::new(),
            cache_ttl: Duration::from_secs(5),
        }
    }

    fn generate_jwt(&self, method: &str, path: &str) -> Result<String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs();

        let uri = format!("{} {}{}", method, "api.coinbase.com", path);

        let claims = JwtClaims {
            sub: self.api_key.clone(),
            iss: "cdp".to_string(),
            nbf: now,
            exp: now + 120,
            uri,
        };

        // The secret is in the format: "-----BEGIN EC PRIVATE KEY-----\n...\n-----END EC PRIVATE KEY-----\n"
        let key = EncodingKey::from_ec_pem(self.api_secret.as_bytes())
            .context("Failed to parse API secret as EC key")?;

        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.api_key.clone());
        header.typ = Some("JWT".to_string());

        encode(&header, &claims, &key).context("Failed to encode JWT")
    }

    async fn rate_limit(&mut self) {
        if let Some(last) = self.last_request {
            let elapsed = last.elapsed();
            if elapsed < MIN_REQUEST_INTERVAL {
                tokio::time::sleep(MIN_REQUEST_INTERVAL - elapsed).await;
            }
        }
        self.last_request = Some(Instant::now());
    }

    pub async fn fetch_ohlcv(
        &mut self,
        timeframe: Timeframe,
        limit: usize,
    ) -> Result<CandleSeries> {
        // Check cache
        let cache_key = format!("{}_{}_{}", self.symbol, timeframe, limit);
        if let Some((cached_at, series)) = self.cache.get(&cache_key) {
            if cached_at.elapsed() < self.cache_ttl {
                return Ok(series.clone());
            }
        }

        self.rate_limit().await;

        let path = format!(
            "/api/v3/brokerage/market/products/{}/candles",
            self.symbol
        );

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs();
        let start = now - (timeframe.as_seconds() * limit as u64);

        let jwt = self.generate_jwt("GET", &path)?;

        let resp = self
            .client
            .get(format!("{}{}", BASE_URL, path))
            .query(&[
                ("start", start.to_string()),
                ("end", now.to_string()),
                ("granularity", timeframe.coinbase_granularity().to_string()),
                ("limit", limit.to_string()),
            ])
            .header("Authorization", format!("Bearer {}", jwt))
            .send()
            .await
            .context("Failed to fetch candles")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Coinbase API error {}: {}", status, body);
        }

        let data: CandleResponse = resp.json().await.context("Failed to parse candle response")?;

        let mut candles: Vec<Candle> = data
            .candles
            .into_iter()
            .filter_map(|rc| {
                let ts = rc.start.parse::<i64>().ok()?;
                let timestamp = DateTime::from_timestamp(ts, 0)?;
                Some(Candle {
                    timestamp,
                    open: rc.open.parse().ok()?,
                    high: rc.high.parse().ok()?,
                    low: rc.low.parse().ok()?,
                    close: rc.close.parse().ok()?,
                    volume: rc.volume.parse().ok()?,
                })
            })
            .collect();

        // Coinbase returns newest first, we want oldest first
        candles.sort_by_key(|c| c.timestamp);

        let series = CandleSeries::new(candles);

        // Update cache
        self.cache
            .insert(cache_key, (Instant::now(), series.clone()));

        Ok(series)
    }

    pub async fn get_current_price(&mut self) -> Result<f64> {
        self.rate_limit().await;

        let path = format!(
            "/api/v3/brokerage/market/products/{}/ticker",
            self.symbol
        );

        let jwt = self.generate_jwt("GET", &path)?;

        let resp = self
            .client
            .get(format!("{}{}", BASE_URL, path))
            .query(&[("limit", "1")])
            .header("Authorization", format!("Bearer {}", jwt))
            .send()
            .await
            .context("Failed to fetch ticker")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Coinbase ticker error {}: {}", status, body);
        }

        let data: TickerResponse = resp.json().await.context("Failed to parse ticker")?;

        data.trades
            .first()
            .and_then(|t| t.price.parse::<f64>().ok())
            .context("No price in ticker response")
    }

    /// Fetch 4H candles by resampling from 1H
    pub async fn get_4h(&mut self, limit: usize) -> Result<CandleSeries> {
        let hours_needed = (limit * 4).min(500);
        let h1 = self.fetch_ohlcv(Timeframe::H1, hours_needed).await?;
        Ok(h1.resample(Duration::from_secs(14400)))
    }

    /// Get midnight (00:00 ET) opening price for today
    pub async fn get_midnight_open(&mut self) -> Result<Option<f64>> {
        let h1 = self.fetch_ohlcv(Timeframe::H1, 48).await?;
        if h1.is_empty() {
            return Ok(None);
        }

        let today = Utc::now().with_timezone(&Eastern).date_naive();

        for candle in h1.iter() {
            let candle_et = candle.timestamp.with_timezone(&Eastern);
            if candle_et.date_naive() == today && candle_et.hour() == 0 {
                return Ok(Some(candle.open));
            }
        }

        // Fallback: first candle of today
        for candle in h1.iter() {
            let candle_et = candle.timestamp.with_timezone(&Eastern);
            if candle_et.date_naive() == today {
                return Ok(Some(candle.open));
            }
        }

        Ok(None)
    }
}

#[async_trait]
impl Exchange for CoinbaseClient {
    async fn fetch_ohlcv(&mut self, tf: Timeframe, limit: usize) -> Result<CandleSeries> {
        self.fetch_ohlcv(tf, limit).await
    }

    async fn get_current_price(&mut self) -> Result<f64> {
        self.get_current_price().await
    }

    async fn get_4h(&mut self, limit: usize) -> Result<CandleSeries> {
        self.get_4h(limit).await
    }

    async fn get_midnight_open(&mut self) -> Result<Option<f64>> {
        self.get_midnight_open().await
    }
}
