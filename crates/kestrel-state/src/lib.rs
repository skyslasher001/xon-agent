//! On-chain state tracking for Kestrel.
//!
//! Maintains a local view of relevant on-chain state:
//! - Token accounts and balances
//! - Pool reserves and prices
//! - Program account states
//! - Watermark-based snapshots for no-lookahead guarantees

use kestrel_core::{CoreResult, Lamports, Pubkey, Slot};
use anyhow::Context;
use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Token account state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenAccount {
    pub mint: Pubkey,
    pub owner: Pubkey,
    pub amount: Lamports,
    pub delegate: Option<Pubkey>,
    pub delegated_amount: Lamports,
    pub state: TokenAccountState,
    pub slot: Slot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TokenAccountState {
    Uninitialized,
    Initialized,
    Frozen,
}

/// Pool state for AMMs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolState {
    pub address: Pubkey,
    pub program_id: Pubkey,
    pub token_a_mint: Pubkey,
    pub token_b_mint: Pubkey,
    pub token_a_vault: Pubkey,
    pub token_b_vault: Pubkey,
    pub reserve_a: Lamports,
    pub reserve_b: Lamports,
    pub fee_bps: u16,
    pub slot: Slot,
}

/// Program account state (generic)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgramAccount {
    pub address: Pubkey,
    pub program_id: Pubkey,
    pub data: Vec<u8>,
    pub slot: Slot,
}

/// Watermarked snapshot for no-lookahead
#[derive(Debug, Clone)]
pub struct StateSnapshot {
    pub slot: Slot,
    pub token_accounts: BTreeMap<Pubkey, TokenAccount>,
    pub pools: BTreeMap<Pubkey, PoolState>,
    pub program_accounts: BTreeMap<Pubkey, ProgramAccount>,
}

/// State tracker with watermark-based snapshots
pub struct StateTracker {
    token_accounts: Arc<DashMap<Pubkey, TokenAccount>>,
    pools: Arc<DashMap<Pubkey, PoolState>>,
    program_accounts: Arc<DashMap<Pubkey, ProgramAccount>>,
    watermark: Arc<RwLock<Slot>>,
}

impl StateTracker {
    pub fn new() -> Self {
        Self {
            token_accounts: Arc::new(DashMap::new()),
            pools: Arc::new(DashMap::new()),
            program_accounts: Arc::new(DashMap::new()),
            watermark: Arc::new(RwLock::new(Slot(0))),
        }
    }

    /// Update watermark (only moves forward)
    pub fn update_watermark(&self, slot: Slot) {
        let mut wm = self.watermark.write();
        if slot > *wm {
            *wm = slot;
        }
    }

    /// Get current watermark
    pub fn watermark(&self) -> Slot {
        *self.watermark.read()
    }

    /// Create a snapshot at current watermark
    pub fn snapshot(&self) -> StateSnapshot {
        let wm = self.watermark();
        StateSnapshot {
            slot: wm,
            token_accounts: self.token_accounts.iter().map(|e| (*e.key(), e.value().clone())).collect(),
            pools: self.pools.iter().map(|e| (*e.key(), e.value().clone())).collect(),
            program_accounts: self.program_accounts.iter().map(|e| (*e.key(), e.value().clone())).collect(),
        }
    }

    /// Update token account
    pub fn upsert_token_account(&self, account: TokenAccount) {
        self.token_accounts.insert(account.mint, account);
    }

    /// Update pool
    pub fn upsert_pool(&self, pool: PoolState) {
        self.pools.insert(pool.address, pool);
    }

    /// Update program account
    pub fn upsert_program_account(&self, account: ProgramAccount) {
        self.program_accounts.insert(account.address, account);
    }

    /// Get token account
    pub fn get_token_account(&self, mint: &Pubkey) -> Option<TokenAccount> {
        self.token_accounts.get(mint).map(|e| e.value().clone())
    }

    /// Get pool
    pub fn get_pool(&self, address: &Pubkey) -> Option<PoolState> {
        self.pools.get(address).map(|e| e.value().clone())
    }

    /// Calculate price from pool reserves
    pub fn pool_price(&self, pool: &Pubkey) -> Option<f64> {
        self.pools.get(pool).map(|p| {
            if p.reserve_a.0 == 0 {
                0.0
            } else {
                p.reserve_b.0 as f64 / p.reserve_a.0 as f64
            }
        })
    }
}

impl Default for StateTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watermark_monotonic() {
        let tracker = StateTracker::new();
        tracker.update_watermark(Slot(100));
        assert_eq!(tracker.watermark(), Slot(100));
        tracker.update_watermark(Slot(50)); // Should not go backward
        assert_eq!(tracker.watermark(), Slot(100));
        tracker.update_watermark(Slot(150));
        assert_eq!(tracker.watermark(), Slot(150));
    }

    #[test]
    fn snapshot_captures_state() {
        let tracker = StateTracker::new();
        let pool = PoolState {
            address: Pubkey::new([1u8; 32]),
            program_id: Pubkey::new([2u8; 32]),
            token_a_mint: Pubkey::new([3u8; 32]),
            token_b_mint: Pubkey::new([4u8; 32]),
            token_a_vault: Pubkey::new([5u8; 32]),
            token_b_vault: Pubkey::new([6u8; 32]),
            reserve_a: Lamports(1_000_000_000),
            reserve_b: Lamports(100_000_000),
            fee_bps: 25,
            slot: Slot(100),
        };
        tracker.upsert_pool(pool);
        tracker.update_watermark(Slot(100));

        let snap = tracker.snapshot();
        assert_eq!(snap.slot, Slot(100));
        assert_eq!(snap.pools.len(), 1);
    }
}