mod bot;

use anyhow::Result;
use tracing_subscriber::{fmt, EnvFilter};

use ict_trading_bot::config::Config;
use ict_trading_bot::exchange::CoinbaseClient;

use crate::bot::IctBot;

#[tokio::main]
async fn main() -> Result<()> {
    let cfg = Config::from_env();

    // Initialize tracing
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(&cfg.log_level));

    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_timer(fmt::time::UtcTime::rfc_3339())
        .init();

    let market = Box::new(CoinbaseClient::new(&cfg));
    let shared_config = cfg.shared();

    let mut bot = IctBot::new(shared_config, market).await;
    bot.run().await?;

    Ok(())
}
