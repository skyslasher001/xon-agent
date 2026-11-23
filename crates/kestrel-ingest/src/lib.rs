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
use tonic::transport::Channel;
use tracing::{debug, error, info, warn};
use yellowstone_grpc_proto::geyser::{
    subscribe_update::UpdateOneof, SubscribeRequest, SubscribeRequestFilterTransactions,
    SubscribeRequestFilterAccounts, SubscribeRequestPing,
};
use yellowstone_grpc_client::GeyserClient;

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
    client: Option<GeyserClient<Channel>>,
}

impl YellowstoneIngest {
    pub fn new(endpoint: String, auth_token: Option<String>) -> Self {
        Self {
            endpoint,
            auth_token,
            client: None,
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
            slots: Some(std::collections::BTreeMap::from([(
                "slots".to_string(),
                SubscribeRequestFilterAccounts::default(),
            )])),
            blocks: None,
            blocks_meta: None,
            entry: None,
            commitment: Some(1), // Confirmed
        }
    }

    async fn connect(&mut self) -> CoreResult<()> {
        let mut endpoint = tonic::transport::Endpoint::from_shared(self.endpoint.clone())
            .context("Invalid gRPC endpoint")?;

        if let Some(token) = &self.auth_token {
            endpoint = endpoint.add_header("x-token", token.parse()?);
        }

        let channel = endpoint.connect().await.context("Failed to connect to Yellowstone gRPC")?;
        let client = GeyserClient::new(channel);
        self.client = Some(client);
        info!("Connected to Yellowstone gRPC at {}", self.endpoint);
        Ok(())
    }
}

#[async_trait::async_trait]
impl IngestSource for YellowstoneIngest {
    async fn start(&mut self) -> CoreResult<Pin<Box<dyn Stream<Item = CoreResult<IngestEvent>> + Send>>> {
        // Connect if not already connected
        if self.client.is_none() {
            self.connect().await?;
        }

        let (tx, rx) = mpsc::unbounded_channel();
        let mut client = self.client.take().context("Client not initialized")?;
        let request = self.build_subscribe_request();

        tokio::spawn(async move {
            if let Err(e) = run_yellowstone_stream(client, request, tx).await {
                error!("Yellowstone stream error: {}", e);
            }
        });

        Ok(Box::pin(rx))
    }

    async fn stop(&mut self) -> CoreResult<()> {
        self.client = None;
        Ok(())
    }

    async fn health_check(&self) -> CoreResult<bool> {
        Ok(self.client.is_some())
    }
}

async fn run_yellowstone_stream(
    mut client: GeyserClient<Channel>,
    request: SubscribeRequest,
    tx: mpsc::UnboundedSender<CoreResult<IngestEvent>>,
) -> CoreResult<()> {
    let mut stream = client.subscribe(request).await.context("Failed to subscribe")?.into_inner();

    while let Some(update) = stream.next().await {
        match update {
            Ok(update) => {
                if let Some(update_oneof) = update.update_oneof {
                    match update_oneof {
                        UpdateOneof::Transaction(tx_update) => {
                            // Parse transaction update
                            let event = parse_transaction_update(tx_update);
                            if let Some(event) = event {
                                if tx.send(Ok(event)).is_err() {
                                    break; // Receiver dropped
                                }
                            }
                        }
                        UpdateOneof::Account(account_update) => {
                            let event = parse_account_update(account_update);
                            if let Some(event) = event {
                                if tx.send(Ok(event)).is_err() {
                                    break;
                                }
                            }
                        }
                        UpdateOneof::Slot(slot_update) => {
                            let event = parse_slot_update(slot_update);
                            if let Some(event) = event {
                                if tx.send(Ok(event)).is_err() {
                                    break;
                                }
                            }
                        }
                        UpdateOneof::Ping(_) => {
                            // Handle ping if needed
                            debug!("Received ping from Yellowstone");
                        }
                        UpdateOneof::Pong(_) => {
                            debug!("Received pong from Yellowstone");
                        }
                        UpdateOneof::Block(_) => {}
                        UpdateOneof::BlockMeta(_) => {}
                        UpdateOneof::Entry(_) => {}
                    }
                }
            }
            Err(e) => {
                error!("Stream error: {}", e);
                if tx.send(Err(e.into())).is_err() {
                    break;
                }
            }
        }
    }

    Ok(())
}

fn parse_transaction_update(tx_update: yellowstone_grpc_proto::geyser::SubscribeUpdateTransaction) -> Option<IngestEvent> {
    let signature = Signature::from_bytes(&tx_update.signature).ok()?;
    let slot = Slot(tx_update.slot);
    let index = tx_update.index;

    let meta = tx_update.meta.map(|m| TransactionMeta {
        fee: m.fee,
        pre_balances: m.pre_balances,
        post_balances: m.post_balances,
        pre_token_balances: m.pre_token_balances.into_iter().map(|tb| TokenBalance {
            account_index: tb.account_index,
            mint: Pubkey::from_bytes(&tb.mint).ok()?,
            ui_amount: tb.ui_amount,
            decimals: tb.decimals as u8,
        }).collect(),
        post_token_balances: m.post_token_balances.into_iter().map(|tb| TokenBalance {
            account_index: tb.account_index,
            mint: Pubkey::from_bytes(&tb.mint).ok()?,
            ui_amount: tb.ui_amount,
            decimals: tb.decimals as u8,
        }).collect(),
        log_messages: m.log_messages,
        err: m.err.map(|e| TransactionError {
            code: e.code,
            message: e.message,
        }),
    });

    Some(IngestEvent::Transaction(TransactionEvent {
        signature,
        slot,
        transaction: tx_update.transaction,
        meta,
        index,
    }))
}

fn parse_account_update(account_update: yellowstone_grpc_proto::geyser::SubscribeUpdateAccount) -> Option<IngestEvent> {
    let pubkey = Pubkey::from_bytes(&account_update.pubkey).ok()?;
    let owner = Pubkey::from_bytes(&account_update.owner).ok()?;

    Some(IngestEvent::AccountUpdate(AccountUpdateEvent {
        pubkey,
        slot: Slot(account_update.slot),
        lamports: account_update.lamports,
        owner,
        executable: account_update.executable,
        rent_epoch: account_update.rent_epoch,
        data: account_update.data,
    }))
}

fn parse_slot_update(slot_update: yellowstone_grpc_proto::geyser::SubscribeUpdateSlot) -> Option<IngestEvent> {
    let status = match slot_update.status {
        1 => SlotStatus::Processed,
        2 => SlotStatus::Confirmed,
        3 => SlotStatus::Finalized,
        _ => SlotStatus::Processed,
    };

    Some(IngestEvent::SlotUpdate(SlotUpdateEvent {
        slot: Slot(slot_update.slot),
        parent: if slot_update.parent > 0 { Some(Slot(slot_update.parent)) } else { None },
        status,
    }))
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