use anyhow::Result;
use chrono::{Duration, Utc};
use tracing_subscriber::{fmt, EnvFilter};

use ict_trading_bot::backtesting::data_fetcher;
use ict_trading_bot::backtesting::BacktestRunner;
use ict_trading_bot::config::Config;
use ict_trading_bot::exchange::HistoricalExchange;
use ict_trading_bot::models::Timeframe;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let cfg = Config::from_env();

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_timer(fmt::time::UtcTime::rfc_3339())
        .init();

    // Parse CLI args or use defaults
    let args: Vec<String> = std::env::args().collect();

    let days_back: i64 = args
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(365);

    let step_minutes: i64 = args
        .get(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);

    let end = Utc::now();
    let start = end - Duration::days(days_back);

    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║          ICT TRADING BOT — BACKTESTER                  ║");
    println!("╠══════════════════════════════════════════════════════════╣");
    println!("║  Symbol:     {}                                  ║", cfg.symbol);
    println!("║  Period:     {} days                               ║", days_back);
    println!("║  Step:       {} minutes                              ║", step_minutes);
    println!("║  Balance:    ${:.2}                              ║", cfg.initial_balance);
    println!("╚══════════════════════════════════════════════════════════╝");
    println!();

    // Timeframes to fetch
    let timeframes = [
        Timeframe::M1,
        Timeframe::M5,
        Timeframe::M15,
        Timeframe::H1,
        Timeframe::H4,
        Timeframe::D1,
    ];

    // Fetch and cache data
    let data_dir = "data";
    let data = data_fetcher::fetch_and_cache(&cfg, start, end, data_dir, &timeframes).await?;

    // Check we have enough data
    let m1_count = data
        .iter()
        .find(|(tf, _)| *tf == Timeframe::M1)
        .map(|(_, c)| c.len())
        .unwrap_or(0);

    if m1_count == 0 {
        println!("ERROR: No 1-minute data available. Cannot backtest.");
        println!("Make sure your Coinbase API credentials are configured in .env");
        return Ok(());
    }

    println!("Data loaded:");
    for (tf, candles) in &data {
        println!("  {}: {} candles", tf, candles.len());
    }
    println!();

    // Build historical exchange
    let mut exchange = HistoricalExchange::new(&cfg.symbol);
    for (tf, candles) in data {
        exchange.load(tf, candles);
    }

    // Determine actual backtest range from available data
    let data_start = exchange.earliest_time().unwrap_or(start);
    let data_end = exchange.latest_time().unwrap_or(end);

    // Start after we have enough lookback (at least 1 day of data)
    let bt_start = data_start + Duration::days(1);
    let bt_end = data_end;

    if bt_start >= bt_end {
        println!("ERROR: Not enough data for backtesting");
        return Ok(());
    }

    println!(
        "Backtesting from {} to {}",
        bt_start.format("%Y-%m-%d %H:%M"),
        bt_end.format("%Y-%m-%d %H:%M")
    );
    println!();

    // Run backtest
    let mut runner = BacktestRunner::new(exchange, cfg);
    let report = runner.run(bt_start, bt_end, step_minutes).await?;

    // Print report
    report.print_summary();

    // Save report to file
    let report_file = format!(
        "data/backtest_{}_{}.txt",
        report.start.format("%Y%m%d"),
        report.end.format("%Y%m%d"),
    );
    save_report_to_file(&report, &report_file)?;
    println!("\nReport saved to: {}", report_file);

    Ok(())
}

fn save_report_to_file(
    report: &ict_trading_bot::backtesting::BacktestReport,
    path: &str,
) -> Result<()> {
    use std::io::Write;

    let mut f = std::fs::File::create(path)?;

    writeln!(f, "ICT Trading Bot Backtest Report")?;
    writeln!(f, "================================")?;
    writeln!(
        f,
        "Period: {} to {} ({:.0} days)",
        report.start.format("%Y-%m-%d"),
        report.end.format("%Y-%m-%d"),
        report.days
    )?;
    writeln!(f)?;
    writeln!(f, "Performance:")?;
    writeln!(f, "  Initial:  ${:.2}", report.initial_balance)?;
    writeln!(f, "  Final:    ${:.2}", report.final_balance)?;
    writeln!(f, "  PnL:      ${:+.2}", report.total_pnl)?;
    writeln!(f, "  Return:   {:+.1}%", report.total_return_pct)?;
    writeln!(f)?;
    writeln!(f, "Trades:")?;
    writeln!(f, "  Total:       {}", report.total_trades)?;
    writeln!(f, "  Win/Loss:    {} / {}", report.winning_trades, report.losing_trades)?;
    writeln!(f, "  Win Rate:    {:.1}%", report.win_rate)?;
    writeln!(f, "  Avg Win:     ${:+.2}", report.avg_win)?;
    writeln!(f, "  Avg Loss:    ${:+.2}", report.avg_loss)?;
    writeln!(f, "  Profit Factor: {:.2}", report.profit_factor)?;
    writeln!(f)?;
    writeln!(f, "Risk:")?;
    writeln!(f, "  Max DD:    ${:.2} ({:.1}%)", report.max_drawdown, report.max_drawdown_pct)?;
    writeln!(f, "  Sharpe:    {:.2}", report.sharpe_ratio)?;
    writeln!(f)?;
    writeln!(f, "Signals:")?;
    writeln!(f, "  Generated: {}", report.total_signals)?;
    writeln!(f, "  Filtered:  {}", report.signals_filtered)?;
    writeln!(f)?;
    writeln!(f, "By Scale:")?;
    for (scale, stats) in &report.scale_stats {
        writeln!(
            f,
            "  {}: {} trades | WR {:.0}% | PnL ${:+.2}",
            scale, stats.trades, stats.win_rate, stats.total_pnl
        )?;
    }
    writeln!(f)?;
    writeln!(f, "By Session:")?;
    for (session, stats) in &report.session_stats {
        writeln!(
            f,
            "  {}: {} trades | WR {:.0}% | PnL ${:+.2}",
            session, stats.trades, stats.win_rate, stats.total_pnl
        )?;
    }

    Ok(())
}
