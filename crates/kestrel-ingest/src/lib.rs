//! Ingestion layer for Kestrel.
//!
//! Handles Yellowstone gRPC streaming (primary) and WebSocket fallback for
//! real-time Solana transaction and account updates.

use kestrel_core::{CoreResult, Pubkey, Signature, Slot};
use anyhow::Context;
use futures::{Stream, StreamExt};
use std::pin::Pin;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};
use yellowstone_grpc_proto::geyser::{
    subscribe_update::UpdateOneof, SubscribeRequest, SubscribeRequestFilterTransactions,
    SubscribeRequestFilterAccounts,
};

/// Ingested event types
#[derive(Debug, Clone)]
pub enum IngestEvent {
    Transaction(TransactionEvent),
    AccountUpdate(AccountUpdateEvent),
    SlotUpdate(SlotUpdateEvent),
}

/// Transaction event from Yellowstone gRPC
#[derive(Debug, Clone)]
pub struct TransactionEvent {
    pub signature: Signature,
    pub slot: Slot,
    pub transaction: Vec<u8>, // Raw transaction bytes
    pub meta: Option<TransactionMeta>,
    pub index: u64,
}

/// Transaction metadata
#[derive(Debug, Clone)]
pub struct TransactionMeta {
    pub fee: u64,
    pub pre_balances: Vec<u64>,
    pub post_balances: Vec<u64>,
    pub pre_token_balances: Vec<TokenBalance>,
    pub post_token_balances: Vec<TokenBalance>,
    pub log_messages: Vec<String>,
    pub err: Option<TransactionError>,
}

/// Token balance
#[derive(Debug, Clone)]
pub struct TokenBalance {
    pub account_index: u32,
    pub mint: Pubkey,
    pub ui_amount: f64,
    pub decimals: u8,
}

/// Transaction error
#[derive(Debug, Clone)]
pub struct TransactionError {
    pub code: i32,
    pub message: String,
}

/// Account update event
#[derive(Debug, Clone)]
pub struct AccountUpdateEvent {
    pub pubkey: Pubkey,
    pub slot: Slot,
    pub lamports: u64,
    pub owner: Pubkey,
    pub executable: bool,
    pub rent_epoch: u64,
    pub data: Vec<u8>,
}

/// Slot update event
#[derive(Debug, Clone)]
pub struct SlotUpdateEvent {
    pub slot: Slot,
    pub parent: Option<Slot>,
    pub status: SlotStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotStatus {
    Processed,
    Confirmed,
    Finalized,
}

/// Ingestion source trait
#[async_trait::async_trait]
pub trait IngestSource: Send + Sync {
    async fn start(&mut self) -> CoreResult<Pin<Box<dyn Stream<Item = CoreResult<IngestEvent>> + Send>>>;
    async fn stop(&mut self) -> CoreResult<()>;
    async fn health_check(&self) -> CoreResult<bool>;
}

/// Yellowstone gRPC ingest source
pub struct YellowstoneIngest {
    endpoint: String,
    auth_token: Option<String>,
    tx_sender: Option<mpsc::UnboundedSender<CoreResult<IngestEvent>>>,
}

impl YellowstoneIngest {
    pub fn new(endpoint: String, auth_token: Option<String>) -> Self {
        Self {
            endpoint,
            auth_token,
            tx_sender: None,
        }
    }

    fn build_subscribe_request(&self) -> SubscribeRequest {
        // Subscribe to Pump.fun and PumpSwap program transactions
        let mut tx_filter = SubscribeRequestFilterTransactions::default();
        tx_filter.account_include = vec![
            "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P".to_string(), // Pump.fun
            "pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA".to_string(), // PumpSwap
        ];
        tx_filter.failed = Some(false);

        SubscribeRequest {
            transactions: std::collections::BTreeMap::from([("pump".to_string(), tx_filter)]),
            accounts: None,
            slots: None,
            blocks: None,
            blocks_meta: None,
            entry: None,
            commitment: Some(1), // Confirmed
        }
    }
}

#[async_trait::async_trait]
impl IngestSource for YellowstoneIngest {
    async fn start(&mut self) -> CoreResult<Pin<Box<dyn Stream<Item = CoreResult<IngestEvent>> + Send>>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.tx_sender = Some(tx.clone());

        let endpoint = self.endpoint.clone();
        let auth_token = self.auth_token.clone();
        let request = self.build_subscribe_request();

        tokio::spawn(async move {
            if let Err(e) = run_yellowstone_stream(endpoint, auth_token, request, tx).await {
                error!("Yellowstone stream error: {}", e);
            }
        });

        Ok(Box::pin(rx))
    }

    async fn stop(&mut self) -> CoreResult<()> {
        self.tx_sender = None;
        Ok(())
    }

    async fn health_check(&self) -> CoreResult<bool> {
        Ok(true) // TODO: Implement actual health check
    }
}

async fn run_yellowstone_stream(
    endpoint: String,
    auth_token: Option<String>,
    request: SubscribeRequest,
    tx: mpsc::UnboundedSender<CoreResult<IngestEvent>>,
) -> CoreResult<()> {
    // TODO: Implement Yellowstone gRPC client
    // This would use yellowstone-grpc-client to connect and stream
    warn!("Yellowstone gRPC client not yet implemented");
    Ok(())
}

/// WebSocket fallback ingest source
pub struct WsIngest {
    url: String,
    tx_sender: Option<mpsc::UnboundedSender<CoreResult<IngestEvent>>>,
}

impl WsIngest {
    pub fn new(url: String) -> Self {
        Self { url, tx_sender: None }
    }
}

#[async_trait::async_trait]
impl IngestSource for WsIngest {
    async fn start(&mut self) -> CoreResult<Pin<Box<dyn Stream<Item = CoreResult<IngestEvent>> + Send>>> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.tx_sender = Some(tx.clone());

        let url = self.url.clone();
        tokio::spawn(async move {
            if let Err(e) = run_ws_stream(url, tx).await {
                error!("WebSocket stream error: {}", e);
            }
        });

        Ok(Box::pin(rx))
    }

    async fn stop(&mut self) -> CoreResult<()> {
        self.tx_sender = None;
        Ok(())
    }

    async fn health_check(&self) -> CoreResult<bool> {
        Ok(true)
    }
}

async fn run_ws_stream(
    url: String,
    tx: mpsc::UnboundedSender<CoreResult<IngestEvent>>,
) -> CoreResult<()> {
    let (ws_stream, _) = connect_async(&url).await.context("Failed to connect to WebSocket")?;
    let (mut write, mut read) = ws_stream.split();

    // Subscribe to logs for Pump.fun
    let subscribe_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "logsSubscribe",
        "params": [
            { "mentions": ["6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P"] },
            { "commitment": "confirmed" }
        ]
    });
    write.send(Message::Text(subscribe_msg.to_string())).await?;

    while let Some(msg) = read.next().await {
        let msg = msg?;
        if let Message::Text(text) = msg {
            // Parse and forward
            debug!("Received WS message: {}", text);
            // TODO: Parse and convert to IngestEvent
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_event_debug() {
        let event = IngestEvent::SlotUpdate(SlotUpdateEvent {
            slot: Slot(100),
            parent: Some(Slot(99)),
            status: SlotStatus::Finalized,
        });
        let debug_str = format!("{:?}", event);
        assert!(debug_str.contains("SlotUpdate"));
    }
}