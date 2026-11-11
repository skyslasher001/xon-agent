//! Ledger-based P&L with FIFO lot accounting for Kestrel.
//!
//! This crate implements precise P&L calculation using FIFO (First In, First Out)
//! lot accounting. All monetary values use integer lamports.

use kestrel_core::{CoreResult, Lamports, Pubkey, Signature, Slot};
use anyhow::Context;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use tracing::{debug, info, warn};

/// A lot (tax lot) representing a position entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lot {
    pub id: u64,
    pub mint: Pubkey,
    pub strategy_id: String,
    pub entry_slot: Slot,
    pub entry_signature: Signature,
    pub quantity: Lamports,
    pub entry_price: Lamports, // Price per unit in lamports
    pub fee_paid: Lamports,
    pub timestamp: DateTime<Utc>,
    pub is_open: bool,
}

/// Position composed of multiple lots
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub mint: Pubkey,
    pub strategy_id: String,
    pub lots: VecDeque<Lot>, // FIFO queue
    pub total_quantity: Lamports,
    pub realized_pnl: Lamports,
    pub unrealized_pnl: Lamports,
}

impl Position {
    pub fn new(mint: Pubkey, strategy_id: String) -> Self {
        Self {
            mint,
            strategy_id,
            lots: VecDeque::new(),
            total_quantity: Lamports(0),
            realized_pnl: Lamports(0),
            unrealized_pnl: Lamports(0),
        }
    }

    /// Add a new lot (entry)
    pub fn add_lot(&mut self, lot: Lot) {
        self.total_quantity = self.total_quantity + lot.quantity;
        self.lots.push_back(lot);
    }

    /// Close lots against an exit (FIFO)
    /// Returns (realized_pnl, remaining_exit_quantity)
    pub fn close_lots(&mut self, exit_price: Lamports, exit_quantity: Lamports, exit_signature: Signature, exit_slot: Slot, fee_paid: Lamports) -> CoreResult<(Lamports, Lamports)> {
        let mut remaining = exit_quantity;
        let mut total_realized = Lamports(0);

        while remaining.0 > 0 && !self.lots.is_empty() {
            let lot = self.lots.front_mut().unwrap();

            let close_qty = std::cmp::min(lot.quantity.0, remaining.0);
            let close_lot = Lamports(close_qty);

            // Calculate P&L for this lot
            // P&L = (exit_price - entry_price) * quantity - fees
            let price_diff = if exit_price.0 > lot.entry_price.0 {
                exit_price.0 - lot.entry_price.0
            } else {
                0 // Can't have negative price diff in this context, fees handle loss
            };
            let gross_pnl = Lamports(price_diff * close_qty);

            // Proportional fee allocation
            let lot_fee = if lot.quantity.0 > 0 {
                Lamports((fee_paid.0 * close_qty) / exit_quantity.0)
            } else {
                Lamports(0)
            };

            let net_pnl = if gross_pnl.0 > lot_fee.0 {
                gross_pnl - lot_fee
            } else {
                Lamports(0)
            };

            total_realized = total_realized + net_pnl;
            lot.quantity = lot.quantity - close_lot;
            remaining = remaining - close_lot;

            // Mark lot as closed if fully consumed
            if lot.quantity.0 == 0 {
                lot.is_open = false;
                self.lots.pop_front();
            }
        }

        self.total_quantity = self.total_quantity - (exit_quantity - remaining);
        self.realized_pnl = self.realized_pnl + total_realized;

        Ok((total_realized, remaining))
    }

    /// Update unrealized P&L based on current price
    pub fn update_unrealized(&mut self, current_price: Lamports) {
        let mut unrealized = Lamports(0);
        for lot in &self.lots {
            if current_price.0 > lot.entry_price.0 {
                let diff = current_price.0 - lot.entry_price.0;
                unrealized = unrealized + Lamports(diff * lot.quantity.0);
            }
        }
        self.unrealized_pnl = unrealized;
    }

    /// Get average entry price (VWAP)
    pub fn avg_entry_price(&self) -> Option<Lamports> {
        if self.total_quantity.0 == 0 {
            return None;
        }

        let mut weighted_sum = 0u128;
        for lot in &self.lots {
            weighted_sum += lot.entry_price.0 as u128 * lot.quantity.0 as u128;
        }

        Some(Lamports((weighted_sum / self.total_quantity.0 as u128) as u64))
    }
}

/// Portfolio P&L aggregator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioPnL {
    pub positions: BTreeMap<Pubkey, Position>,
    pub total_realized: Lamports,
    pub total_unrealized: Lamports,
    pub total_fees: Lamports,
    pub daily_pnl: BTreeMap<String, Lamports>, // Date -> P&L
}

impl PortfolioPnL {
    pub fn new() -> Self {
        Self {
            positions: BTreeMap::new(),
            total_realized: Lamports(0),
            total_unrealized: Lamports(0),
            total_fees: Lamports(0),
            daily_pnl: BTreeMap::new(),
        }
    }

    pub fn record_entry(&mut self, mint: Pubkey, strategy_id: String, quantity: Lamports, entry_price: Lamports, entry_signature: Signature, entry_slot: Slot, fee_paid: Lamports) {
        let position = self.positions.entry(mint).or_insert_with(|| Position::new(mint, strategy_id.clone()));
        let lot = Lot {
            id: rand::random(),
            mint,
            strategy_id,
            entry_slot,
            entry_signature,
            quantity,
            entry_price,
            fee_paid,
            timestamp: Utc::now(),
            is_open: true,
        };
        position.add_lot(lot);
        self.total_fees = self.total_fees + fee_paid;
    }

    pub fn record_exit(&mut self, mint: Pubkey, exit_price: Lamports, exit_quantity: Lamports, exit_signature: Signature, exit_slot: Slot, fee_paid: Lamports) -> CoreResult<Lamports> {
        let position = self.positions.get_mut(&mint).context("Position not found")?;
        let (realized, remaining) = position.close_lots(exit_price, exit_quantity, exit_signature, exit_slot, fee_paid)?;

        if remaining.0 > 0 {
            warn!("Partial fill: {} remaining", remaining.0);
        }

        self.total_realized = self.total_realized + realized;
        self.total_fees = self.total_fees + fee_paid;

        // Update daily P&L
        let date = Utc::now().format("%Y-%m-%d").to_string();
        *self.daily_pnl.entry(date).or_insert(Lamports(0)) += realized;

        Ok(realized)
    }

    pub fn update_prices(&mut self, prices: &BTreeMap<Pubkey, Lamports>) {
        self.total_unrealized = Lamports(0);
        for (mint, price) in prices {
            if let Some(position) = self.positions.get_mut(mint) {
                position.update_unrealized(*price);
                self.total_unrealized = self.total_unrealized + position.unrealized_pnl;
            }
        }
    }

    pub fn total_pnl(&self) -> Lamports {
        self.total_realized + self.total_unrealized
    }

    pub fn get_position(&self, mint: &Pubkey) -> Option<&Position> {
        self.positions.get(mint)
    }

    pub fn open_positions(&self) -> Vec<&Position> {
        self.positions.values().filter(|p| p.total_quantity.0 > 0).collect()
    }
}

impl Default for PortfolioPnL {
    fn default() -> Self {
        Self::new()
    }
}

/// P&L report for a time period
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PnLReport {
    pub period_start: DateTime<Utc>,
    pub period_end: DateTime<Utc>,
    pub realized: Lamports,
    pub unrealized: Lamports,
    pub fees: Lamports,
    pub net: Lamports,
    pub trades: u64,
    pub win_rate: f32,
    pub profit_factor: f32,
    pub max_drawdown: Lamports,
    pub sharpe: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use kestrel_core::{Lamports, Pubkey, Signature, Slot};

    #[test]
    fn lot_fifo_accounting() {
        let mut position = Position::new(Pubkey::new([1u8; 32]), "test".to_string());

        // Buy 100 at 10
        position.add_lot(Lot {
            id: 1,
            mint: Pubkey::new([1u8; 32]),
            strategy_id: "test".to_string(),
            entry_slot: Slot(100),
            entry_signature: Signature([0u8; 64]),
            quantity: Lamports(100),
            entry_price: Lamports(10),
            fee_paid: Lamports(1),
            timestamp: Utc::now(),
            is_open: true,
        });

        // Buy 50 at 12
        position.add_lot(Lot {
            id: 2,
            mint: Pubkey::new([1u8; 32]),
            strategy_id: "test".to_string(),
            entry_slot: Slot(101),
            entry_signature: Signature([1u8; 64]),
            quantity: Lamports(50),
            entry_price: Lamports(12),
            fee_paid: Lamports(1),
            timestamp: Utc::now(),
            is_open: true,
        });

        // Sell 120 at 15 (should close first lot completely, second partially)
        let (realized, remaining) = position.close_lots(Lamports(15), Lamports(120), Signature([2u8; 64]), Slot(102), Lamports(2)).unwrap();

        assert_eq!(remaining.0, 0);
        // First lot: 100 * (15-10) = 500, fee = 2 * 100/120 = 1.67 -> net ~498
        // Second lot: 20 * (15-12) = 60, fee = 2 * 20/120 = 0.33 -> net ~60
        // Total ~558
        assert!(realized.0 > 500 && realized.0 < 600);
    }

    #[test]
    fn portfolio_pnl() {
        let mut pnl = PortfolioPnL::new();
        let mint = Pubkey::new([1u8; 32]);

        pnl.record_entry(mint, "test".to_string(), Lamports(100), Lamports(10), Signature([0u8; 64]), Slot(100), Lamports(1));
        let realized = pnl.record_exit(mint, Lamports(15), Lamports(100), Signature([1u8; 64]), Slot(101), Lamports(1)).unwrap();

        assert_eq!(pnl.total_realized, realized);
        assert_eq!(pnl.total_fees, Lamports(2));
    }
}