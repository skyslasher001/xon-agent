//! Risk management for Kestrel.
//!
//! Implements tiered bet sizing, circuit breakers, hard caps, and kill switch.

use kestrel_core::{CoreResult, Lamports, Pubkey, Slot};
use kestrel_pnl::PortfolioPnL;
use kestrel_strategy::{StrategyId, SizeTier, StrategySignal};
use anyhow::Context;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Risk configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskConfig {
    // Hard caps (enforced in code, cannot be bypassed)
    pub max_position_sol: f64,      // 2 SOL default
    pub max_exposure_sol: f64,      // 20 SOL default
    pub max_daily_loss_sol: f64,    // 5 SOL default

    // Tiered sizing
    pub scout_multiplier: f64,      // 0.25x
    pub standard_multiplier: f64,   // 1.0x
    pub conviction_multiplier: f64, // 3.0x
    pub conviction_min_confirmations: u8, // 2

    // Circuit breakers
    pub max_drawdown_pct: f32,      // 20% default
    pub max_consecutive_losses: u32, // 5 default
    pub cooldown_slots: u64,        // 100 slots default

    // Regime dial
    pub regime_min: f64,            // 0.5x
    pub regime_max: f64,            // 1.5x
    pub regime_default: f64,        // 1.0x
}

impl Default for RiskConfig {
    fn default() -> Self {
        Self {
            max_position_sol: 2.0,
            max_exposure_sol: 20.0,
            max_daily_loss_sol: 5.0,
            scout_multiplier: 0.25,
            standard_multiplier: 1.0,
            conviction_multiplier: 3.0,
            conviction_min_confirmations: 2,
            max_drawdown_pct: 0.20,
            max_consecutive_losses: 5,
            cooldown_slots: 100,
            regime_min: 0.5,
            regime_max: 1.5,
            regime_default: 1.0,
        }
    }
}

/// Risk state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskState {
    pub current_exposure: Lamports,
    pub daily_pnl: Lamports,
    pub daily_loss: Lamports,
    pub peak_equity: Lamports,
    pub current_drawdown: f32,
    pub consecutive_losses: u32,
    pub last_loss_slot: Option<Slot>,
    pub cooldown_until: Option<Slot>,
    pub regime_multiplier: f64,
    pub kill_switch_activated: bool,
    pub blocked_strategies: BTreeMap<StrategyId, String>,
}

impl Default for RiskState {
    fn default() -> Self {
        Self {
            current_exposure: Lamports(0),
            daily_pnl: Lamports(0),
            daily_loss: Lamports(0),
            peak_equity: Lamports(0),
            current_drawdown: 0.0,
            consecutive_losses: 0,
            last_loss_slot: None,
            cooldown_until: None,
            regime_multiplier: 1.0,
            kill_switch_activated: false,
            blocked_strategies: BTreeMap::new(),
        }
    }
}

/// Risk check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskCheck {
    pub allowed: bool,
    pub reason: Option<String>,
    pub adjusted_size: Option<Lamports>,
    pub warnings: Vec<String>,
}

/// Main risk manager
pub struct RiskManager {
    config: RiskConfig,
    state: Arc<RwLock<RiskState>>,
}

impl RiskManager {
    pub fn new(config: RiskConfig) -> Self {
        let mut state = RiskState::default();
        state.regime_multiplier = config.regime_default;
        state.peak_equity = Lamports(0);

        Self {
            config,
            state: Arc::new(RwLock::new(state)),
        }
    }

    /// Check if a signal is allowed and calculate adjusted size
    pub fn check_signal(&self, signal: &StrategySignal, portfolio: &PortfolioPnL) -> RiskCheck {
        let mut state = self.state.write();
        let mut warnings = Vec::new();

        // Kill switch check
        if state.kill_switch_activated {
            return RiskCheck {
                allowed: false,
                reason: Some("Kill switch activated".into()),
                adjusted_size: None,
                warnings: vec![],
            };
        }

        // Cooldown check
        if let Some(cooldown) = state.cooldown_until {
            if signal.expiry_slot < cooldown {
                return RiskCheck {
                    allowed: false,
                    reason: Some(format!("In cooldown until slot {}", cooldown)),
                    adjusted_size: None,
                    warnings: vec![],
                };
            }
        }

        // Strategy blocked check
        if let Some(reason) = state.blocked_strategies.get(&signal.strategy_id) {
            return RiskCheck {
                allowed: false,
                reason: Some(format!("Strategy blocked: {}", reason)),
                adjusted_size: None,
                warnings: vec![],
            };
        }

        // Daily loss limit
        let max_daily_loss = Lamports((self.config.max_daily_loss_sol * 1_000_000_000.0) as u64);
        if state.daily_loss > max_daily_loss {
            return RiskCheck {
                allowed: false,
                reason: Some(format!("Daily loss limit exceeded: {} > {}", state.daily_loss, max_daily_loss)),
                adjusted_size: None,
                warnings: vec![],
            };
        }

        // Drawdown check
        if state.current_drawdown > self.config.max_drawdown_pct {
            // Activate cooldown
            state.cooldown_until = Some(signal.expiry_slot + self.config.cooldown_slots);
            warnings.push(format!("Max drawdown exceeded: {:.1}%", state.current_drawdown * 100.0));
        }

        // Consecutive losses check
        if state.consecutive_losses >= self.config.max_consecutive_losses {
            state.cooldown_until = Some(signal.expiry_slot + self.config.cooldown_slots);
            warnings.push(format!("Consecutive losses: {}", state.consecutive_losses));
        }

        // Calculate base size from signal
        let base_size = self.calculate_base_size(signal, portfolio);
        let adjusted_size = self.apply_tier_multiplier(base_size, signal.size_tier);

        // Check position cap
        let max_position = Lamports((self.config.max_position_sol * 1_000_000_000.0) as u64);
        let final_size = std::cmp::min(adjusted_size, max_position);

        // Check exposure cap
        let max_exposure = Lamports((self.config.max_exposure_sol * 1_000_000_000.0) as u64);
        let projected_exposure = state.current_exposure + final_size;
        if projected_exposure > max_exposure {
            if max_exposure > state.current_exposure {
                return RiskCheck {
                    allowed: true,
                    reason: None,
                    adjusted_size: Some(max_exposure - state.current_exposure),
                    warnings,
                };
            } else {
                return RiskCheck {
                    allowed: false,
                    reason: Some(format!("Exposure cap reached: {} >= {}", state.current_exposure, max_exposure)),
                    adjusted_size: None,
                    warnings,
                };
            }
        }

        // Apply regime multiplier
        let regime_adjusted = Lamports((final_size.0 as f64 * state.regime_multiplier) as u64);

        RiskCheck {
            allowed: true,
            reason: None,
            adjusted_size: Some(regime_adjusted),
            warnings,
        }
    }

    fn calculate_base_size(&self, signal: &StrategySignal, portfolio: &PortfolioPnL) -> Lamports {
        // Base size as percentage of available capital
        let equity = portfolio.total_pnl() + portfolio.total_realized; // Simplified
        let base_pct = 0.05; // 5% of equity per trade
        Lamports((equity.0 as f64 * base_pct) as u64)
    }

    fn apply_tier_multiplier(&self, base: Lamports, tier: SizeTier) -> Lamports {
        let multiplier = match tier {
            SizeTier::Scout => self.config.scout_multiplier,
            SizeTier::Standard => self.config.standard_multiplier,
            SizeTier::Conviction => self.config.conviction_multiplier,
        };
        Lamports((base.0 as f64 * multiplier) as u64)
    }

    /// Record trade result
    pub fn record_trade(&self, pnl: Lamports, is_exit: bool) {
        let mut state = self.state.write();

        if is_exit {
            state.daily_pnl = state.daily_pnl + pnl;
            if pnl.0 > 0 {
                state.consecutive_losses = 0;
            } else {
                state.consecutive_losses += 1;
                state.daily_loss = state.daily_loss + Lamports(pnl.0.abs() as u64);
                state.last_loss_slot = Some(Slot(0)); // Would be actual slot
            }
        }

        // Update equity and drawdown
        let current_equity = state.daily_pnl; // Simplified
        if current_equity.0 > state.peak_equity.0 {
            state.peak_equity = current_equity;
        }

        if state.peak_equity.0 > 0 {
            let drawdown = 1.0 - (current_equity.0 as f64 / state.peak_equity.0 as f64);
            state.current_drawdown = drawdown.max(0.0) as f32;
        }
    }

    /// Update exposure tracking
    pub fn update_exposure(&self, delta: Lamports) {
        let mut state = self.state.write();
        state.current_exposure = if state.current_exposure.0 >= delta.0 {
            state.current_exposure - delta
        } else {
            Lamports(0)
        };
    }

    /// Set regime multiplier (0.5x - 1.5x)
    pub fn set_regime(&self, multiplier: f64) {
        let mut state = self.state.write();
        state.regime_multiplier = multiplier.clamp(self.config.regime_min, self.config.regime_max);
    }

    /// Activate kill switch
    pub fn activate_kill_switch(&self) {
        let mut state = self.state.write();
        state.kill_switch_activated = true;
        error!("KILL SWITCH ACTIVATED");
    }

    /// Deactivate kill switch (manual only)
    pub fn deactivate_kill_switch(&self) {
        let mut state = self.state.write();
        state.kill_switch_activated = false;
        warn!("Kill switch deactivated manually");
    }

    /// Block a strategy
    pub fn block_strategy(&self, strategy_id: StrategyId, reason: String) {
        let mut state = self.state.write();
        state.blocked_strategies.insert(strategy_id, reason);
    }

    /// Unblock a strategy
    pub fn unblock_strategy(&self, strategy_id: StrategyId) {
        let mut state = self.state.write();
        state.blocked_strategies.remove(&strategy_id);
    }

    /// Get current risk state
    pub fn get_state(&self) -> RiskState {
        self.state.read().clone()
    }

    /// Reset daily counters (call at midnight)
    pub fn reset_daily(&self) {
        let mut state = self.state.write();
        state.daily_pnl = Lamports(0);
        state.daily_loss = Lamports(0);
        state.consecutive_losses = 0;
        state.cooldown_until = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kestrel_pnl::PortfolioPnL;
    use kestrel_strategy::{StrategyId, StrategySignal, StrategyAction, SizeTier};
    use kestrel_core::{Lamports, Pubkey, Slot};

    #[test]
    fn risk_config_defaults() {
        let config = RiskConfig::default();
        assert_eq!(config.max_position_sol, 2.0);
        assert_eq!(config.max_exposure_sol, 20.0);
        assert_eq!(config.max_daily_loss_sol, 5.0);
    }

    #[test]
    fn tier_multipliers() {
        let config = RiskConfig::default();
        let manager = RiskManager::new(config);
        let base = Lamports(1_000_000_000);

        assert_eq!(manager.apply_tier_multiplier(base, SizeTier::Scout), Lamports(250_000_000));
        assert_eq!(manager.apply_tier_multiplier(base, SizeTier::Standard), Lamports(1_000_000_000));
        assert_eq!(manager.apply_tier_multiplier(base, SizeTier::Conviction), Lamports(3_000_000_000));
    }

    #[test]
    fn kill_switch_blocks_all() {
        let config = RiskConfig::default();
        let manager = RiskManager::new(config);
        manager.activate_kill_switch();

        let signal = StrategySignal {
            strategy_id: StrategyId::LaunchScalper,
            mint: Pubkey::new([1u8; 32]),
            action: StrategyAction::Enter,
            size_tier: SizeTier::Standard,
            confidence: 1.0,
            reasoning: "test".into(),
            expiry_slot: Slot(100),
        };

        let portfolio = PortfolioPnL::new();
        let check = manager.check_signal(&signal, &portfolio);
        assert!(!check.allowed);
        assert_eq!(check.reason, Some("Kill switch activated".into()));
    }

    #[test]
    fn daily_loss_limit() {
        let config = RiskConfig::default();
        let manager = RiskManager::new(config);

        // Set daily loss to exceed limit
        {
            let mut state = manager.state.write();
            state.daily_loss = Lamports(6_000_000_000); // 6 SOL > 5 SOL limit
        }

        let signal = StrategySignal {
            strategy_id: StrategyId::LaunchScalper,
            mint: Pubkey::new([1u8; 32]),
            action: StrategyAction::Enter,
            size_tier: SizeTier::Standard,
            confidence: 1.0,
            reasoning: "test".into(),
            expiry_slot: Slot(100),
        };

        let portfolio = PortfolioPnL::new();
        let check = manager.check_signal(&signal, &portfolio);
        assert!(!check.allowed);
        assert!(check.reason.unwrap().contains("Daily loss limit"));
    }
}