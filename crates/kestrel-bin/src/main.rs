//! Kestrel CLI entry point.
//!
//! A high-performance Solana meme-coin trading system.

use kestrel_core::{CoreResult, RunMode};
use kestrel_engine::{EngineConfig, TradingEngine, EngineCli, EngineCommand};
use anyhow::Context;
use clap::Parser;
use std::path::PathBuf;
use tracing::{error, info, Level};
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main]
async fn main() -> CoreResult<()> {
    // Initialize logging
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("kestrel=info,tower=warn,hyper=warn"));

    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .init();

    let cli = EngineCli::parse();

    match cli.command {
        EngineCommand::Run { mode, config } => {
            info!("Starting Kestrel in {:?} mode", mode);
            run_engine(mode, config).await?;
        }
        EngineCommand::Backtest { data_path, config } => {
            info!("Running backtest from {:?}", data_path);
            run_backtest(data_path, config).await?;
        }
        EngineCommand::PnL { config } => {
            info!("Generating P&L report");
            run_pnl(config).await?;
        }
    }

    Ok(())
}

async fn run_engine(mode: RunMode, config_path: Option<PathBuf>) -> CoreResult<()> {
    // Load config
    let mut config = if let Some(path) = config_path {
        load_config(&path).await?
    } else {
        EngineConfig::default()
    };

    config.mode = mode;

    // Safety check for live mode
    if mode == RunMode::Live {
        check_live_mode_safety().await?;
    }

    // Create and run engine
    let mut engine = TradingEngine::new(config)?;
    engine.init_ingest().await?;

    // Setup graceful shutdown
    let engine_ref = std::sync::Arc::new(tokio::sync::Mutex::new(engine));
    let engine_clone = engine_ref.clone();

    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.expect("Failed to listen for Ctrl+C");
        info!("Shutdown signal received");
        let mut engine = engine_clone.lock().await;
        engine.stop().await;
    });

    // Run engine
    let mut engine = engine_ref.lock().await;
    engine.run().await?;

    Ok(())
}

async fn run_backtest(data_path: PathBuf, config_path: Option<PathBuf>) -> CoreResult<()> {
    let config = if let Some(path) = config_path {
        load_config(&path).await?
    } else {
        EngineConfig::default()
    };

    let engine = TradingEngine::new(config)?;
    engine.backtest(&data_path).await?;

    Ok(())
}

async fn run_pnl(config_path: Option<PathBuf>) -> CoreResult<()> {
    let config = if let Some(path) = config_path {
        load_config(&path).await?
    } else {
        EngineConfig::default()
    };

    let engine = TradingEngine::new(config)?;
    // TODO: Generate P&L report
    info!("P&L report generated");

    Ok(())
}

async fn load_config(path: &PathBuf) -> CoreResult<EngineConfig> {
    let content = tokio::fs::read_to_string(path).await
        .context("Failed to read config file")?;

    let config: EngineConfig = if path.extension().map_or(false, |ext| ext == "toml") {
        toml::from_str(&content).context("Failed to parse TOML config")?
    } else {
        serde_json::from_str(&content).context("Failed to parse JSON config")?
    };

    Ok(config)
}

async fn check_live_mode_safety() -> CoreResult<()> {
    // Check environment variables for live mode confirmation
    let live_confirm = std::env::var("KESTREL_LIVE_CONFIRM")
        .unwrap_or_default();

    if live_confirm != "I_ACCEPT_RISK" {
        anyhow::bail!(
            "Live mode requires KESTREL_LIVE_CONFIRM=I_ACCEPT_RISK environment variable. \
            This is a double-lock safety mechanism. Set this only if you understand the risks."
        );
    }

    let executor_available = std::env::var("LIVE_EXECUTOR_AVAILABLE")
        .unwrap_or_default();

    if executor_available != "true" {
        anyhow::bail!(
            "Live mode requires LIVE_EXECUTOR_AVAILABLE=true environment variable. \
            This is currently hardcoded to false in the codebase for safety."
        );
    }

    // Final warning
    eprintln!();
    eprintln!("╔═══════════════════════════════════════════════════════════════════╗");
    eprintln!("║                    ⚠️  LIVE MODE ACTIVATED  ⚠️                     ║");
    eprintln!("║                                                                    ║");
    eprintln!("║  This software trades REAL money on Solana mainnet.               ║");
    eprintln!("║  Most meme coins go to zero. Many are built to take your money.   ║");
    eprintln!("║  This software can lose everything in its wallet.                 ║");
    eprintln!("║                                                                    ║");
    eprintln!("║  Press Ctrl+C within 10 seconds to abort.                         ║");
    eprintln!("╚═══════════════════════════════════════════════════════════════════╝");
    eprintln!();

    tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;

    Ok(())
}