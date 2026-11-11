//! Main trading engine orchestration for Kestrel.
//!
//! Coordinates all components: ingest → state → safety → scorer → strategy → risk → portfolio → exec

use kestrel_core::{CoreResult, Lamports, Pubkey, Slot, Signature};
use kestrel_ingest::{IngestEvent, IngestSource, YellowstoneIngest, WsIngest};
use kestrel_state::StateTracker;
use kestrel_safety::{SafetyAnalyzer, SafetyConfig, SafetyReport, SafetyVerdict};
use kestrel_scorer::{Scorer, ScorerConfig, TokenScore};
use kestrel_strategy::{StrategyRegistry, StrategySignal, StrategyId, LaunchScalper, StrategyConfig};
use kestrel_risk::{RiskManager, RiskConfig, RiskCheck};
use kestrel_portfolio::{PortfolioManager, PortfolioConfig, OrderRequest};
use kestrel_exec::{ExecutionEngine, ExecutionResult, ExecutionReport, JitoClient, RpcClient, TransactionSigner, Side, Urgency, VenueClient};
use kestrel_venues::{Venue, JupiterClient, OKXDEXClient, PumpFunClient};
use kestrel_pnl::PortfolioPnL;
use anyhow::Context;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use tracing::{debug, error, info, warn, instrument};

/// Engine run mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
pub enum RunMode {
    Paper,
    Backtest,
    Live,
}

/// Engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    pub mode: RunMode,
    pub ingest: IngestConfig,
    pub safety: SafetyConfig,
    pub scorer: ScorerConfig,
    pub risk: RiskConfig,
    pub portfolio: PortfolioConfig,
    pub execution: ExecutionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestConfig {
    pub yellowstone_url: String,
    pub yellowstone_token: Option<String>,
    pub ws_url: String,
    pub use_yellowstone: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionConfig {
    pub jito_url: String,
    pub jito_auth: String,
    pub rpc_urls: Vec<String>,
    pub signer_type: SignerType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignerType {
    KeypairFile,
    EnvVar,
    HardwareWallet,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            mode: RunMode::Paper,
            ingest: IngestConfig {
                yellowstone_url: "http://localhost:10000".into(),
                yellowstone_token: None,
                ws_url: "wss://api.mainnet-beta.solana.com".into(),
                use_yellowstone: true,
            },
            safety: SafetyConfig::default(),
            scorer: ScorerConfig::default(),
            risk: RiskConfig::default(),
            portfolio: PortfolioConfig::default(),
            execution: ExecutionConfig {
                jito_url: "https://mainnet.block-engine.jito.wtf".into(),
                jito_auth: "".into(),
                rpc_urls: vec!["https://api.mainnet-beta.solana.com".into()],
                signer_type: SignerType::EnvVar,
            },
        }
    }
}

/// Main trading engine
pub struct TradingEngine {
    config: EngineConfig,
    state: Arc<StateTracker>,
    safety: Arc<SafetyAnalyzer>,
    scorer: Arc<Scorer>,
    strategies: Arc<tokio::sync::RwLock<StrategyRegistry>>,
    risk: Arc<RiskManager>,
    portfolio: Arc<PortfolioManager>,
    execution: Arc<ExecutionEngine>,
    ingest: Option<Box<dyn IngestSource>>,
    running: Arc<tokio::sync::RwLock<bool>>,
    event_tx: mpsc::UnboundedSender<IngestEvent>,
    event_rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<IngestEvent>>>,
}

impl TradingEngine {
    pub fn new(config: EngineConfig) -> CoreResult<Self> {
        let state = Arc::new(StateTracker::new());
        let safety = Arc::new(SafetyAnalyzer::new(config.safety.clone(), state.clone())?);
        let scorer = Arc::new(Scorer::new(config.scorer.clone(), state.clone()));
        let strategies = Arc::new(tokio::sync::RwLock::new(StrategyRegistry::default()));
        let risk = Arc::new(RiskManager::new(config.risk.clone()));
        let portfolio = Arc::new(PortfolioManager::new(
            config.portfolio.clone(),
            risk.clone(),
            strategies.clone(),
        ));

        // Build venues
        let mut venues: BTreeMap<Venue, Arc<dyn VenueClient>> = BTreeMap::new();
        venues.insert(Venue::Jupiter, Arc::new(JupiterClient::new("https://quote-api.jup.ag/v6".into())));
        venues.insert(Venue::OKXDEX, Arc::new(OKXDEXClient::new("".into(), "".into())));
        venues.insert(Venue::PumpFun, Arc::new(PumpFunClient::new("https://pump.fun".into())));

        // Build execution engine
        let signer: Arc<dyn TransactionSigner> = Arc::new(EnvSigner::new()?);
        let jito = Arc::new(JitoClient::new(config.execution.jito_url.clone(), config.execution.jito_auth.clone()));
        let rpc_clients: Vec<Arc<RpcClient>> = config.execution.rpc_urls.iter()
            .map(|url| Arc::new(RpcClient::new(url.clone())))
            .collect();

        let execution = Arc::new(ExecutionEngine::new(venues, signer, Some(jito), rpc_clients));

        let (event_tx, event_rx) = mpsc::unbounded_channel();

        Ok(Self {
            config,
            state,
            safety,
            scorer,
            strategies,
            risk,
            portfolio,
            execution,
            ingest: None,
            running: Arc::new(tokio::sync::RwLock::new(false)),
            event_tx,
            event_rx: Arc::new(tokio::sync::Mutex::new(event_rx)),
        })
    }

    /// Initialize ingest source
    pub async fn init_ingest(&mut self) -> CoreResult<()> {
        let ingest: Box<dyn IngestSource> = if self.config.ingest.use_yellowstone {
            Box::new(YellowstoneIngest::new(
                self.config.ingest.yellowstone_url.clone(),
                self.config.ingest.yellowstone_token.clone(),
            ))
        } else {
            Box::new(WsIngest::new(self.config.ingest.ws_url.clone()))
        };

        self.ingest = Some(ingest);
        Ok(())
    }

    /// Start the engine
    #[instrument(skip(self))]
    pub async fn run(&mut self) -> CoreResult<()> {
        *self.running.write().await = true;
        info!("Starting Kestrel engine in {:?} mode", self.config.mode);

        // Start ingest
        if let Some(ingest) = &mut self.ingest {
            let mut stream = ingest.start().await?;
            let event_tx = self.event_tx.clone();
            let running = self.running.clone();

            tokio::spawn(async move {
                while *running.read().await {
                    if let Some(event) = stream.next().await {
                        if let Ok(event) = event {
                            if event_tx.send(event).is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }

        // Main event loop
        let mut price_update_interval = interval(Duration::from_secs(10));
        let mut snapshot_interval = interval(Duration::from_secs(60));
        let mut rebalance_interval = interval(Duration::from_secs(300));

        while *self.running.read().await {
            tokio::select! {
                // Process ingested events
                event = self.event_rx.lock().await.recv() => {
                    if let Some(event) = event {
                        if let Err(e) = self.process_event(event).await {
                            error!("Event processing error: {}", e);
                        }
                    }
                }

                // Periodic price updates
                _ = price_update_interval.tick() => {
                    self.update_prices().await;
                }

                // Periodic snapshots
                _ = snapshot_interval.tick() => {
                    let snapshot = self.portfolio.snapshot();
                    info!("Portfolio snapshot: equity={}, positions={}", snapshot.total_equity, snapshot.positions.len());
                }

                // Periodic rebalance
                _ = rebalance_interval.tick() => {
                    self.portfolio.rebalance();
                }
            }
        }

        info!("Engine stopped");
        Ok(())
    }

    /// Stop the engine
    pub async fn stop(&self) {
        *self.running.write().await = false;
        if let Some(ingest) = &mut self.ingest {
            let _ = ingest.stop().await;
        }
    }

    /// Process a single ingest event
    async fn process_event(&self, event: IngestEvent) -> CoreResult<()> {
        match event {
            IngestEvent::Transaction(tx_event) => {
                self.process_transaction(tx_event).await?;
            }
            IngestEvent::AccountUpdate(acc_event) => {
                self.process_account_update(acc_event).await?;
            }
            IngestEvent::SlotUpdate(slot_event) => {
                self.process_slot_update(slot_event).await?;
            }
        }
        Ok(())
    }

    /// Process transaction event
    async fn process_transaction(&self, tx_event: kestrel_ingest::TransactionEvent) -> CoreResult<()> {
        // Update state watermark
        self.state.update_watermark(tx_event.slot);

        // Extract token mints from transaction
        // TODO: Parse transaction for new token launches, swaps, etc.
        debug!("Processing transaction {} at slot {}", tx_event.signature, tx_event.slot.0);

        // For new launches, run safety + scoring + strategy evaluation
        // This is a simplified version - real implementation would parse the transaction
        // to identify new mints and significant events
        Ok(())
    }

    /// Process account update
    async fn process_account_update(&self, acc_event: kestrel_ingest::AccountUpdateEvent) -> CoreResult<()> {
        // Update state tracker
        // TODO: Parse account data based on program
        debug!("Account update: {} at slot {}", acc_event.pubkey, acc_event.slot.0);
        Ok(())
    }

    /// Process slot update
    async fn process_slot_update(&self, slot_event: kestrel_ingest::SlotUpdateEvent) -> CoreResult<()> {
        self.portfolio.update_slot(slot_event.slot);
        self.state.update_watermark(slot_event.slot);
        Ok(())
    }

    /// Evaluate a new token mint
    pub async fn evaluate_mint(&self, mint: Pubkey, slot: Slot) -> CoreResult<()> {
        // 1. Safety analysis
        let safety = self.safety.analyze(&mint, slot).await?;

        // Block if safety verdict is Block
        if safety.overall_verdict == SafetyVerdict::Block {
            info!("Token {} blocked by safety: {}", mint, safety.checks.iter()
                .filter(|c| c.verdict == SafetyVerdict::Block)
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>()
                .join(", "));
            return Ok(());
        }

        // 2. Get pool state
        let pools: Vec<_> = self.state.pools.iter()
            .filter(|p| p.value().token_a_mint == mint || p.value().token_b_mint == mint)
            .map(|p| p.value().clone())
            .collect();

        // 3. Score the token
        let score = self.scorer.score(&safety, &pools).await?;

        // 4. Check if scorer allows entry
        if !self.scorer.should_enter(&score) {
            debug!("Token {} below entry threshold: {}", mint, score.composite);
            return Ok(());
        }

        // 5. Strategy evaluation
        let strategies = self.strategies.read().await;
        let signals = strategies.evaluate_all(&score, &self.state).await?;

        // 6. Process signals through risk and portfolio
        for signal in signals {
            if let Some(order) = self.portfolio.process_signal(&signal).await? {
                // Execute order
                let report = self.execution.execute_batch(vec![order]).await?;
                for result in report.results {
                    self.portfolio.record_execution(&OrderRequest {
                        id: result.order_id.clone(),
                        strategy_id: signal.strategy_id.as_str().to_string(),
                        mint: signal.mint,
                        side: if signal.action == kestrel_strategy::StrategyAction::Exit {
                            Side::Sell
                        } else {
                            Side::Buy
                        },
                        quantity: signal.size_tier.into(),
                        max_price: None,
                        venue_preference: vec![],
                        urgency: Urgency::High,
                        client_id: String::new(),
                    }, &result);
                }
            }
        }

        Ok(())
    }

    /// Update mark prices from venues
    async fn update_prices(&self) {
        // TODO: Fetch prices from venues and update portfolio
        let prices = BTreeMap::new();
        self.portfolio.update_prices(&prices);
    }

    /// Run backtest
    pub async fn backtest(&self, data_path: &std::path::Path) -> CoreResult<()> {
        info!("Running backtest from {:?}", data_path);
        // TODO: Implement backtest
        Ok(())
    }
}

/// Environment variable signer
struct EnvSigner {
    keypair: solana_sdk::signature::Keypair,
}

impl EnvSigner {
    fn new() -> CoreResult<Self> {
        let keypair = solana_sdk::signature::Keypair::new(); // Placeholder
        Ok(Self { keypair })
    }
}

impl TransactionSigner for EnvSigner {
    fn sign_transaction(&self, message: &[u8]) -> CoreResult<Signature> {
        let sig = self.keypair.sign_message(message);
        Ok(Signature(*sig.as_ref()))
    }

    fn pubkey(&self) -> Pubkey {
        Pubkey(*self.keypair.pubkey().as_ref())
    }

    fn sign_versioned_transaction(&self, tx: &solana_sdk::transaction::VersionedTransaction) -> CoreResult<solana_sdk::transaction::VersionedTransaction> {
        // TODO: Sign versioned transaction
        Ok(tx.clone())
    }
}

impl From<SizeTier> for Lamports {
    fn from(tier: SizeTier) -> Self {
        match tier {
            SizeTier::Scout => Lamports(250_000_000),
            SizeTier::Standard => Lamports(1_000_000_000),
            SizeTier::Conviction => Lamports(3_000_000_000),
        }
    }
}

/// CLI for the engine
#[derive(Parser)]
#[command(name = "kestrel-engine")]
#[command(about = "Kestrel trading engine")]
pub struct EngineCli {
    #[command(subcommand)]
    pub command: EngineCommand,
}

#[derive(Subcommand)]
pub enum EngineCommand {
    Run {
        #[arg(long, default_value = "paper")]
        mode: RunMode,
        #[arg(long)]
        config: Option<std::path::PathBuf>,
    },
    Backtest {
        #[arg(long)]
        data_path: std::path::PathBuf,
        #[arg(long)]
        config: Option<std::path::PathBuf>,
    },
    PnL {
        #[arg(long)]
        config: Option<std::path::PathBuf>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_config_default() {
        let config = EngineConfig::default();
        assert_eq!(config.mode, RunMode::Paper);
    }

    #[test]
    fn run_mode_serialization() {
        let mode = RunMode::Paper;
        let json = serde_json::to_string(&mode).unwrap();
        assert_eq!(json, "\"Paper\"");
    }
}