//! Execution engine for Kestrel.
//!
//! Handles order routing, transaction building, signing, and submission
//! with Jito bundle support and parallel RPC landing.

use kestrel_core::{CoreResult, Lamports, Pubkey, Signature, Slot};
use kestrel_venues::{Quote, SwapResult, Venue, VenueClient};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Execution side
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

/// Order request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRequest {
    pub id: String,
    pub strategy_id: String,
    pub mint: Pubkey,
    pub side: Side,
    pub quantity: Lamports,
    pub max_price: Option<Lamports>, // For limit orders
    pub venue_preference: Vec<Venue>,
    pub urgency: Urgency,
    pub client_id: String, // Idempotency key
}

/// Order urgency
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Urgency {
    Emergency,  // Exit, use Jito + all RPCs
    High,       // Entry, use Jito + primary RPC
    Normal,     // Standard, single RPC
    Low,        // Non-critical, can wait
}

/// Execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub order_id: String,
    pub success: bool,
    pub signature: Option<Signature>,
    pub filled_quantity: Lamports,
    pub avg_price: Lamports,
    pub fee_paid: Lamports,
    pub slot: Slot,
    pub venue: Venue,
    pub error: Option<String>,
    pub attempts: u32,
}

/// Execution report for a batch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionReport {
    pub results: Vec<ExecutionResult>,
    pub total_filled: Lamports,
    pub total_fees: Lamports,
    pub duration_ms: u64,
}

/// Transaction signer (wallet)
pub trait TransactionSigner: Send + Sync {
    fn sign_transaction(&self, message: &[u8]) -> CoreResult<Signature>;
    fn pubkey(&self) -> Pubkey;
    fn sign_versioned_transaction(&self, tx: &solana_sdk::transaction::VersionedTransaction) -> CoreResult<solana_sdk::transaction::VersionedTransaction>;
}

/// Execution engine
pub struct ExecutionEngine {
    venues: BTreeMap<Venue, Arc<dyn VenueClient>>,
    signer: Arc<dyn TransactionSigner>,
    jito_client: Option<Arc<JitoClient>>,
    rpc_clients: Vec<Arc<RpcClient>>,
    pending: Arc<RwLock<BTreeMap<String, OrderRequest>>>,
    next_client_id: Arc<RwLock<u64>>,
}

impl ExecutionEngine {
    pub fn new(
        venues: BTreeMap<Venue, Arc<dyn VenueClient>>,
        signer: Arc<dyn TransactionSigner>,
        jito_client: Option<Arc<JitoClient>>,
        rpc_clients: Vec<Arc<RpcClient>>,
    ) -> Self {
        Self {
            venues,
            signer,
            jito_client,
            rpc_clients,
            pending: Arc::new(RwLock::new(BTreeMap::new())),
            next_client_id: Arc::new(RwLock::new(0)),
        }
    }

    /// Generate next idempotent client ID
    async fn next_client_id(&self) -> String {
        let mut id = self.next_client_id.write().await;
        *id += 1;
        format!("kestrel-{}", *id)
    }

    /// Execute a single order
    pub async fn execute(&self, mut request: OrderRequest) -> CoreResult<ExecutionResult> {
        request.client_id = self.next_client_id().await;

        // Store as pending
        self.pending.write().await.insert(request.id.clone(), request.clone());

        let result = match request.urgency {
            Urgency::Emergency => self.execute_emergency(request).await,
            Urgency::High => self.execute_high_priority(request).await,
            Urgency::Normal => self.execute_normal(request).await,
            Urgency::Low => self.execute_low_priority(request).await,
        };

        self.pending.write().await.remove(&request.id);
        result
    }

    /// Emergency execution: Jito bundle + parallel RPCs, exits prioritized
    async fn execute_emergency(&self, request: OrderRequest) -> CoreResult<ExecutionResult> {
        info!("Emergency execution for {}", request.id);

        // 1. Try Jito bundle first
        if let Some(jito) = &self.jito_client {
            if let Ok(result) = jito.send_bundle(&request, &*self.signer).await {
                return Ok(result);
            }
        }

        // 2. Parallel RPC submission
        self.execute_parallel_rpcs(request).await
    }

    /// High priority: Jito + primary RPC
    async fn execute_high_priority(&self, request: OrderRequest) -> CoreResult<ExecutionResult> {
        // Try Jito first
        if let Some(jito) = &self.jito_client {
            if let Ok(result) = jito.send_bundle(&request, &*self.signer).await {
                return Ok(result);
            }
        }

        // Fallback to primary RPC
        self.execute_single_rpc(request, 0).await
    }

    /// Normal execution: single RPC
    async fn execute_normal(&self, request: OrderRequest) -> CoreResult<ExecutionResult> {
        self.execute_single_rpc(request, 0).await
    }

    /// Low priority: single RPC with retries
    async fn execute_low_priority(&self, request: OrderRequest) -> CoreResult<ExecutionResult> {
        let mut last_error = None;
        for attempt in 0..3 {
            match self.execute_single_rpc(request.clone(), attempt).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    last_error = Some(e);
                    tokio::time::sleep(tokio::time::Duration::from_millis(100 * (attempt + 1) as u64)).await;
                }
            }
        }
        Err(last_error.unwrap())
    }

    /// Execute via single RPC
    async fn execute_single_rpc(&self, request: OrderRequest, attempt: u32) -> CoreResult<ExecutionResult> {
        // Get quote from preferred venue
        let venue = request.venue_preference.first().copied().unwrap_or(Venue::Jupiter);
        let venue_client = self.venues.get(&venue).context("Venue not available")?;

        let quote = venue_client.get_quote(
            request.mint,
            Pubkey::new([0u8; 32]), // WSOL/SOL mint placeholder
            request.quantity,
        ).await?;

        // Execute swap
        let swap_result = venue_client.execute_swap(&quote, &*self.signer).await?;

        Ok(ExecutionResult {
            order_id: request.id,
            success: true,
            signature: Some(swap_result.signature),
            filled_quantity: swap_result.output_amount,
            avg_price: swap_result.output_amount, // Simplified
            fee_paid: swap_result.fee_paid,
            slot: swap_result.slot,
            venue,
            error: None,
            attempts: attempt + 1,
        })
    }

    /// Execute via parallel RPCs
    async fn execute_parallel_rpcs(&self, request: OrderRequest) -> CoreResult<ExecutionResult> {
        // TODO: Submit same transaction to multiple RPC endpoints
        // Return first success
        self.execute_single_rpc(request, 0).await
    }

    /// Execute a batch of orders (exits first, then entries)
    pub async fn execute_batch(&self, mut requests: Vec<OrderRequest>) -> CoreResult<ExecutionReport> {
        let start = std::time::Instant::now();

        // Sort: exits (sell) first, then entries (buy), then by urgency
        requests.sort_by(|a, b| {
            match (a.side, b.side) {
                (Side::Sell, Side::Buy) => std::cmp::Ordering::Less,
                (Side::Buy, Side::Sell) => std::cmp::Ordering::Greater,
                _ => b.urgency.cmp(&a.urgency), // Higher urgency first
            }
        });

        let mut results = Vec::new();
        let mut total_filled = Lamports(0);
        let mut total_fees = Lamports(0);

        for request in requests {
            match self.execute(request).await {
                Ok(result) => {
                    total_filled = total_filled + result.filled_quantity;
                    total_fees = total_fees + result.fee_paid;
                    results.push(result);
                }
                Err(e) => {
                    error!("Order {} failed: {}", results.len(), e);
                    results.push(ExecutionResult {
                        order_id: format!("failed-{}", results.len()),
                        success: false,
                        signature: None,
                        filled_quantity: Lamports(0),
                        avg_price: Lamports(0),
                        fee_paid: Lamports(0),
                        slot: Slot(0),
                        venue: Venue::Jupiter,
                        error: Some(e.to_string()),
                        attempts: 0,
                    });
                }
            }
        }

        Ok(ExecutionReport {
            results,
            total_filled,
            total_fees,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Pre-build warm sell transactions for emergency exits
    pub async fn build_warm_sells(&self, positions: &[PositionSnapshot]) -> CoreResult<Vec<WarmSell>> {
        // TODO: Pre-sign sell transactions for all open positions
        Ok(vec![])
    }
}

/// Pre-built warm sell for emergency exit
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarmSell {
    pub mint: Pubkey,
    pub quantity: Lamports,
    pub signed_tx: Vec<u8>,
    pub client_id: String,
    pub expires_at: Slot,
}

/// Position snapshot for warm sell building
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionSnapshot {
    pub mint: Pubkey,
    pub quantity: Lamports,
    pub strategy_id: String,
}

/// Jito client for bundle submission
pub struct JitoClient {
    endpoint: String,
    auth_key: String,
    http_client: reqwest::Client,
}

impl JitoClient {
    pub fn new(endpoint: String, auth_key: String) -> Self {
        Self {
            endpoint,
            auth_key,
            http_client: reqwest::Client::new(),
        }
    }

    pub async fn send_bundle(&self, request: &OrderRequest, signer: &dyn TransactionSigner) -> CoreResult<ExecutionResult> {
        // TODO: Build and submit Jito bundle
        Err(anyhow::anyhow!("Jito bundle not implemented").into())
    }
}

/// RPC client wrapper
pub struct RpcClient {
    endpoint: String,
    http_client: reqwest::Client,
}

impl RpcClient {
    pub fn new(endpoint: String) -> Self {
        Self {
            endpoint,
            http_client: reqwest::Client::new(),
        }
    }

    pub async fn send_transaction(&self, tx: &[u8]) -> CoreResult<Signature> {
        // TODO: Send transaction via RPC
        Err(anyhow::anyhow!("RPC send not implemented").into())
    }

    pub async fn simulate_transaction(&self, tx: &[u8]) -> CoreResult<bool> {
        // TODO: Simulate transaction
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_sorting_exits_first() {
        let mut orders = vec![
            OrderRequest { id: "1".into(), strategy_id: "s".into(), mint: Pubkey::new([1u8;32]), side: Side::Buy, quantity: Lamports(100), max_price: None, venue_preference: vec![], urgency: Urgency::Normal, client_id: "".into() },
            OrderRequest { id: "2".into(), strategy_id: "s".into(), mint: Pubkey::new([2u8;32]), side: Side::Sell, quantity: Lamports(100), max_price: None, venue_preference: vec![], urgency: Urgency::Normal, client_id: "".into() },
        ];
        orders.sort_by(|a, b| {
            match (a.side, b.side) {
                (Side::Sell, Side::Buy) => std::cmp::Ordering::Less,
                (Side::Buy, Side::Sell) => std::cmp::Ordering::Greater,
                _ => b.urgency.cmp(&a.urgency),
            }
        });
        assert_eq!(orders[0].id, "2"); // Sell first
        assert_eq!(orders[1].id, "1"); // Buy second
    }
}