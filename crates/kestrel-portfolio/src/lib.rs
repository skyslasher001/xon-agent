//! Portfolio management for Kestrel.
//!
//! Coordinates positions, P&L, risk, and strategy allocations.

use kestrel_core::{CoreResult, Lamports, Pubkey, Slot, Signature};
use kestrel_pnl::{PortfolioPnL, Position, PnLReport};
use kestrel_risk::{RiskManager, RiskCheck, RiskState};
use kestrel_strategy::{StrategyId, StrategySignal, StrategyAction, SizeTier, StrategyRegistry};
use anyhow::Context;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Portfolio configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioConfig {
    pub initial_capital_sol: f64,
    pub max_strategies: usize,
    pub rebalance_interval_slots: u64,
}

impl Default for PortfolioConfig {
    fn default() -> Self {
        Self {
            initial_capital_sol: 50.0,
            max_strategies: 5,
            rebalance_interval_slots: 1000,
        }
    }
}

/// Portfolio state snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortfolioSnapshot {
    pub timestamp: Slot,
    pub total_equity: Lamports,
    pub available_capital: Lamports,
    pub deployed_capital: Lamports,
    pub unrealized_pnl: Lamports,
    pub realized_pnl: Lamports,
    pub daily_pnl: Lamports,
    pub positions: BTreeMap<Pubkey, PositionSnapshot>,
    pub strategy_allocations: BTreeMap<StrategyId, StrategyAllocation>,
    pub risk_state: RiskState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionSnapshot {
    pub mint: Pubkey,
    pub strategy_id: StrategyId,
    pub quantity: Lamports,
    pub entry_price: Lamports,
    pub current_price: Lamports,
    pub unrealized_pnl: Lamports,
    pub size_tier: SizeTier,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyAllocation {
    pub strategy_id: StrategyId,
    pub allocated_sol: f64,
    pub deployed_sol: f64,
    pub pnl_sol: f64,
    pub trade_count: u64,
    pub win_rate: f32,
}

/// Main portfolio manager
pub struct PortfolioManager {
    config: PortfolioConfig,
    pnl: Arc<RwLock<PortfolioPnL>>,
    risk: Arc<RiskManager>,
    strategies: Arc<RwLock<StrategyRegistry>>,
    strategy_allocations: Arc<RwLock<BTreeMap<StrategyId, f64>>>,
    current_slot: Arc<RwLock<Slot>>,
}

impl PortfolioManager {
    pub fn new(config: PortfolioConfig, risk: Arc<RiskManager>, strategies: Arc<RwLock<StrategyRegistry>>) -> Self {
        let pnl = Arc::new(RwLock::new(PortfolioPnL::new()));
        let mut allocations = BTreeMap::new();

        // Initialize equal allocations
        let strat_count = strategies.read().strategies.len().max(1);
        let per_strategy = config.initial_capital_sol / strat_count as f64;
        for id in strategies.read().strategies.keys() {
            allocations.insert(*id, per_strategy);
        }

        Self {
            config,
            pnl,
            risk,
            strategies,
            strategy_allocations: Arc::new(RwLock::new(allocations)),
            current_slot: Arc::new(RwLock::new(Slot(0))),
        }
    }

    /// Update current slot
    pub fn update_slot(&self, slot: Slot) {
        *self.current_slot.write() = slot;
    }

    /// Process a strategy signal
    pub async fn process_signal(&self, signal: &StrategySignal) -> CoreResult<Option<OrderRequest>> {
        // Check risk
        let pnl = self.pnl.read().clone();
        let risk_check = self.risk.check_signal(signal, &pnl);

        if !risk_check.allowed {
            info!("Signal rejected by risk: {:?}", risk_check.reason);
            return Ok(None);
        }

        let size = risk_check.adjusted_size.unwrap_or(Lamports(0));
        if size.0 == 0 {
            return Ok(None);
        }

        // Create order request
        let side = match signal.action {
            StrategyAction::Enter | StrategyAction::Increase => kestrel_exec::Side::Buy,
            StrategyAction::Exit | StrategyAction::Decrease => kestrel_exec::Side::Sell,
            StrategyAction::Hold => return Ok(None),
        };

        let order = OrderRequest {
            id: format!("{}-{}-{}", signal.strategy_id.as_str(), signal.mint, *self.current_slot.read()),
            strategy_id: signal.strategy_id.as_str().to_string(),
            mint: signal.mint,
            side,
            quantity: size,
            max_price: None, // Market order for now
            venue_preference: vec![kestrel_venues::Venue::Jupiter, kestrel_venues::Venue::OKXDEX],
            urgency: match signal.action {
                StrategyAction::Exit => kestrel_exec::Urgency::Emergency,
                _ => kestrel_exec::Urgency::High,
            },
            client_id: String::new(), // Will be set by execution engine
        };

        Ok(Some(order))
    }

    /// Record trade execution result
    pub fn record_execution(&self, order: &OrderRequest, result: &kestrel_exec::ExecutionResult) {
        let mut pnl = self.pnl.write();

        if result.success {
            if order.side == kestrel_exec::Side::Buy {
                pnl.record_entry(
                    order.mint,
                    order.strategy_id.clone(),
                    result.filled_quantity,
                    result.avg_price,
                    result.signature.unwrap_or(Signature([0u8; 64])),
                    result.slot,
                    result.fee_paid,
                );
                self.risk.update_exposure(result.filled_quantity);
            } else {
                let realized = pnl.record_exit(
                    order.mint,
                    result.avg_price,
                    result.filled_quantity,
                    result.signature.unwrap_or(Signature([0u8; 64])),
                    result.slot,
                    result.fee_paid,
                ).unwrap_or(Lamports(0));

                self.risk.record_trade(realized, true);
                self.risk.update_exposure(Lamports(0)); // Simplified
            }
        }
    }

    /// Update mark prices for unrealized P&L
    pub fn update_prices(&self, prices: &BTreeMap<Pubkey, Lamports>) {
        let mut pnl = self.pnl.write();
        pnl.update_prices(prices);
    }

    /// Get portfolio snapshot
    pub fn snapshot(&self) -> PortfolioSnapshot {
        let pnl = self.pnl.read();
        let risk_state = self.risk.get_state();
        let strategies = self.strategies.read();
        let allocations = self.strategy_allocations.read();

        let mut positions = BTreeMap::new();
        for (mint, pos) in &pnl.positions {
            if pos.total_quantity.0 > 0 {
                let avg_price = pos.avg_entry_price().unwrap_or(Lamports(0));
                positions.insert(*mint, PositionSnapshot {
                    mint: *mint,
                    strategy_id: StrategyId::LaunchScalper, // Simplified
                    quantity: pos.total_quantity,
                    entry_price: avg_price,
                    current_price: Lamports(0), // Would come from price feed
                    unrealized_pnl: pos.unrealized_pnl,
                    size_tier: SizeTier::Standard,
                });
            }
        }

        let mut strategy_allocations = BTreeMap::new();
        for (id, allocated) in allocations.iter() {
            if let Some(strat) = strategies.strategies.get(id) {
                let state = strat.state();
                strategy_allocations.insert(*id, StrategyAllocation {
                    strategy_id: *id,
                    allocated_sol: *allocated,
                    deployed_sol: state.current_exposure.to_sol(),
                    pnl_sol: state.total_pnl.to_sol(),
                    trade_count: state.total_trades,
                    win_rate: if state.total_trades > 0 {
                        state.winning_trades as f32 / state.total_trades as f32
                    } else {
                        0.0
                    },
                });
            }
        }

        PortfolioSnapshot {
            timestamp: *self.current_slot.read(),
            total_equity: pnl.total_pnl(),
            available_capital: Lamports((self.config.initial_capital_sol * 1_000_000_000.0) as u64) - pnl.total_realized, // Simplified
            deployed_capital: risk_state.current_exposure,
            unrealized_pnl: pnl.total_unrealized,
            realized_pnl: pnl.total_realized,
            daily_pnl: risk_state.daily_pnl,
            positions,
            strategy_allocations,
            risk_state,
        }
    }

    /// Generate P&L report
    pub fn generate_report(&self, period_start: Slot, period_end: Slot) -> PnLReport {
        let pnl = self.pnl.read();
        // Simplified report
        PnLReport {
            period_start: chrono::Utc::now(),
            period_end: chrono::Utc::now(),
            realized: pnl.total_realized,
            unrealized: pnl.total_unrealized,
            fees: pnl.total_fees,
            net: pnl.total_pnl(),
            trades: pnl.positions.values().map(|p| p.lots.len() as u64).sum(),
            win_rate: 0.0,
            profit_factor: 0.0,
            max_drawdown: Lamports(0),
            sharpe: 0.0,
        }
    }

    /// Rebalance strategy allocations
    pub fn rebalance(&self) {
        let mut allocations = self.strategy_allocations.write();
        let strategies = self.strategies.read();

        // Simple rebalance: adjust based on performance
        let total_pnl: i64 = strategies.strategies.values()
            .map(|s| s.state().total_pnl.0 as i64)
            .sum();

        if total_pnl != 0 {
            for (id, allocated) in allocations.iter_mut() {
                if let Some(strat) = strategies.strategies.get(id) {
                    let strat_pnl = strat.state().total_pnl.0 as i64;
                    let performance = strat_pnl as f64 / total_pnl as f64;
                    // Adjust allocation by up to 20% based on performance
                    let adjustment = (performance - 1.0 / allocations.len() as f64) * 0.2;
                    *allocated = (*allocated * (1.0 + adjustment)).max(0.0);
                }
            }
        }
    }
}

/// Order request for execution engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRequest {
    pub id: String,
    pub strategy_id: String,
    pub mint: Pubkey,
    pub side: kestrel_exec::Side,
    pub quantity: Lamports,
    pub max_price: Option<Lamports>,
    pub venue_preference: Vec<kestrel_venues::Venue>,
    pub urgency: kestrel_exec::Urgency,
    pub client_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use kestrel_risk::RiskConfig;
    use kestrel_strategy::{StrategyRegistry, LaunchScalper, StrategyConfig, StrategyId, StrategySignal, StrategyAction, SizeTier};
    use kestrel_core::{Lamports, Pubkey, Slot, Signature};
    use std::sync::Arc;

    #[test]
    fn portfolio_creation() {
        let config = PortfolioConfig::default();
        let risk = Arc::new(kestrel_risk::RiskManager::new(RiskConfig::default()));
        let strategies = Arc::new(parking_lot::RwLock::new(StrategyRegistry::default()));

        let portfolio = PortfolioManager::new(config, risk, strategies);

        let snapshot = portfolio.snapshot();
        assert_eq!(snapshot.total_equity.0, 0);
    }
}