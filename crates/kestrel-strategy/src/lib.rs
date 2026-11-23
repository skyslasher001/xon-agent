//! Strategy framework for Kestrel.
//!
//! Pluggable strategy system with promotion gates:
//! - Launch Scalper (core earner, sub-minute)
//! - Curve Momentum
//! - Graduation Play
//! - Smart-Money Follow
//! - Volume Breakout

use kestrel_core::{CoreResult, Lamports, Pubkey, Slot, Signature};
use kestrel_safety::{SafetyReport, SafetyVerdict};
use kestrel_scorer::{TokenScore, Signal, SignalSource};
use kestrel_state::StateTracker;
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use async_trait::async_trait;
use tracing::{debug, info, warn};

mod curve_momentum;

/// Strategy identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum StrategyId {
    LaunchScalper,
    CurveMomentum,
    GraduationPlay,
    SmartMoneyFollow,
    VolumeBreakout,
}

impl StrategyId {
    pub fn as_str(&self) -> &'static str {
        match self {
            StrategyId::LaunchScalper => "launch_scalper",
            StrategyId::CurveMomentum => "curve_momentum",
            StrategyId::GraduationPlay => "graduation_play",
            StrategyId::SmartMoneyFollow => "smart_money_follow",
            StrategyId::VolumeBreakout => "volume_breakout",
        }
    }
}

/// Strategy signal (action to take)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySignal {
    pub strategy_id: StrategyId,
    pub mint: Pubkey,
    pub action: StrategyAction,
    pub size_tier: SizeTier,
    pub confidence: f32,
    pub reasoning: String,
    pub expiry_slot: Slot,
}

/// Strategy action
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrategyAction {
    Enter,
    Exit,
    Increase,
    Decrease,
    Hold,
}

/// Position size tier
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SizeTier {
    Scout,      // 0.25x base
    Standard,   // 1.0x base
    Conviction, // up to 3.0x base (requires ≥2 confirmations)
}

/// Strategy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyConfig {
    pub enabled: bool,
    pub budget_sol: f64,           // Max SOL allocation
    pub max_position_sol: f64,     // Max single position
    pub max_daily_loss_sol: f64,   // Daily loss limit
    pub min_score: f32,            // Minimum composite score
    pub size_tier: SizeTier,
    pub conviction_confirmations: u8, // Required for Conviction tier
    pub custom_params: BTreeMap<String, serde_json::Value>,
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            budget_sol: 10.0,
            max_position_sol: 2.0,
            max_daily_loss_sol: 5.0,
            min_score: 60.0,
            size_tier: SizeTier::Standard,
            conviction_confirmations: 2,
            custom_params: BTreeMap::new(),
        }
    }
}

/// Strategy state for tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyState {
    pub strategy_id: StrategyId,
    pub total_trades: u64,
    pub winning_trades: u64,
    pub total_pnl: Lamports,
    pub daily_pnl: Lamports,
    pub open_positions: u32,
    pub current_exposure: Lamports,
    pub is_paused: bool,
    pub promotion_status: PromotionStatus,
}

/// Promotion status for paper → live
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PromotionStatus {
    Paper,           // Running in paper mode
    Qualifying,      // Meeting paper requirements
    Promoted,        // Promoted to live
    Rejected,        // Failed promotion criteria
}

/// Strategy trait
#[async_trait]
pub trait Strategy: Send + Sync {
    fn id(&self) -> StrategyId;
    fn config(&self) -> &StrategyConfig;
    fn config_mut(&mut self) -> &mut StrategyConfig;
    fn state(&self) -> &StrategyState;
    fn state_mut(&mut self) -> &mut StrategyState;

    /// Evaluate a token and produce signals
    async fn evaluate(&self, score: &TokenScore, state: &StateTracker) -> CoreResult<Vec<StrategySignal>>;

    /// Check if strategy should enter a position
    fn should_enter(&self, score: &TokenScore) -> bool {
        score.composite >= self.config().min_score
            && self.state().current_exposure.0 < (self.config().max_position_sol * 1_000_000_000.0) as u64
            && !self.state().is_paused
    }

    /// Calculate position size based on tier
    fn calculate_size(&self, base_size: Lamports, confirmations: u8) -> Lamports {
        let multiplier = match (self.config().size_tier, confirmations) {
            (SizeTier::Scout, _) => 0.25,
            (SizeTier::Standard, _) => 1.0,
            (SizeTier::Conviction, c) if c >= self.config().conviction_confirmations => 3.0,
            (SizeTier::Conviction, _) => 1.0, // Fallback to standard
        };
        Lamports((base_size.0 as f64 * multiplier) as u64)
    }

    /// Update state after trade
    fn record_trade(&mut self, pnl: Lamports, is_win: bool) {
        let state = self.state_mut();
        state.total_trades += 1;
        if is_win {
            state.winning_trades += 1;
        }
        state.total_pnl = state.total_pnl + pnl;
    }

    /// Check promotion criteria
    fn check_promotion(&mut self) -> PromotionStatus {
        let state = self.state();
        if state.total_trades >= 150
            && state.total_pnl.0 > 0
            && (state.winning_trades as f64 / state.total_trades as f64) > 0.5
        {
            PromotionStatus::Promoted
        } else {
            PromotionStatus::Paper
        }
    }
}

/// Launch Scalper - core earner, sub-minute holds
pub struct LaunchScalper {
    config: StrategyConfig,
    state: StrategyState,
}

impl LaunchScalper {
    pub fn new(config: StrategyConfig) -> Self {
        Self {
            config,
            state: StrategyState {
                strategy_id: StrategyId::LaunchScalper,
                total_trades: 0,
                winning_trades: 0,
                total_pnl: Lamports(0),
                daily_pnl: Lamports(0),
                open_positions: 0,
                current_exposure: Lamports(0),
                is_paused: false,
                promotion_status: PromotionStatus::Paper,
            },
        }
    }
}

#[async_trait]
impl Strategy for LaunchScalper {
    fn id(&self) -> StrategyId {
        StrategyId::LaunchScalper
    }

    fn config(&self) -> &StrategyConfig {
        &self.config
    }

    fn config_mut(&mut self) -> &mut StrategyConfig {
        &mut self.config
    }

    fn state(&self) -> &StrategyState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut StrategyState {
        &mut self.state
    }

    async fn evaluate(&self, score: &TokenScore, _state: &StateTracker) -> CoreResult<Vec<StrategySignal>> {
        let mut signals = Vec::new();

        // Check for launch scalper signals
        let has_momentum = score.signals.iter().any(|s| s.name == "strong_momentum");
        let has_liquidity = score.signals.iter().any(|s| s.name == "high_liquidity");
        let safety_clean = score.safety.overall_verdict == SafetyVerdict::Clean;

        if self.should_enter(score) && has_momentum && has_liquidity && safety_clean {
            signals.push(StrategySignal {
                strategy_id: StrategyId::LaunchScalper,
                mint: score.mint,
                action: StrategyAction::Enter,
                size_tier: self.config.size_tier,
                confidence: 0.8,
                reasoning: "Launch scalper: strong momentum + high liquidity + clean safety".into(),
                expiry_slot: score.slot + 10, // ~10 slots = ~4 seconds
            });
        }

        // Exit signal on momentum loss
        if score.components.momentum_score < 0.3 {
            signals.push(StrategySignal {
                strategy_id: StrategyId::LaunchScalper,
                mint: score.mint,
                action: StrategyAction::Exit,
                size_tier: SizeTier::Standard,
                confidence: 0.9,
                reasoning: "Momentum lost".into(),
                expiry_slot: score.slot + 5,
            });
        }

        Ok(signals)
    }
}

/// Curve Momentum strategy (placeholder)
pub struct CurveMomentum {
    config: StrategyConfig,
    state: StrategyState,
}

impl CurveMomentum {
    pub fn new(config: StrategyConfig) -> Self {
        Self {
            config,
            state: StrategyState {
                strategy_id: StrategyId::CurveMomentum,
                total_trades: 0,
                winning_trades: 0,
                total_pnl: Lamports(0),
                daily_pnl: Lamports(0),
                open_positions: 0,
                current_exposure: Lamports(0),
                is_paused: false,
                promotion_status: PromotionStatus::Paper,
            },
        }
    }
}

#[async_trait]
impl Strategy for CurveMomentum {
    fn id(&self) -> StrategyId {
        StrategyId::CurveMomentum
    }

    fn config(&self) -> &StrategyConfig {
        &self.config
    }

    fn config_mut(&mut self) -> &mut StrategyConfig {
        &mut self.config
    }

    fn state(&self) -> &StrategyState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut StrategyState {
        &mut self.state
    }

    async fn evaluate(&self, _score: &TokenScore, _state: &StateTracker) -> CoreResult<Vec<StrategySignal>> {
        // TODO: Implement curve momentum logic
        Ok(vec![])
    }
}

/// Strategy registry for managing all strategies
pub struct StrategyRegistry {
    strategies: BTreeMap<StrategyId, Box<dyn Strategy>>,
}

impl StrategyRegistry {
    pub fn new() -> Self {
        Self {
            strategies: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, strategy: Box<dyn Strategy>) {
        self.strategies.insert(strategy.id(), strategy);
    }

    pub fn get(&self, id: StrategyId) -> Option<&dyn Strategy> {
        self.strategies.get(&id).map(|s| s.as_ref())
    }

    pub fn get_mut(&mut self, id: StrategyId) -> Option<&mut dyn Strategy> {
        self.strategies.get_mut(&id).map(|s| s.as_mut())
    }

    pub async fn evaluate_all(&self, score: &TokenScore, state: &StateTracker) -> CoreResult<Vec<StrategySignal>> {
        let mut all_signals = Vec::new();
        for strategy in self.strategies.values() {
            if strategy.config().enabled {
                let signals = strategy.evaluate(score, state).await?;
                all_signals.extend(signals);
            }
        }
        Ok(all_signals)
    }

    pub fn check_promotions(&mut self) {
        for strategy in self.strategies.values_mut() {
            let status = strategy.check_promotion();
            strategy.state_mut().promotion_status = status;
        }
    }
}

impl Default for StrategyRegistry {
    fn default() -> Self {
        let mut registry = Self::new();
        registry.register(Box::new(LaunchScalper::new(StrategyConfig::default())));
        registry.register(Box::new(CurveMomentum::new(StrategyConfig::default())));
        registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_tier_calculation() {
        let config = StrategyConfig::default();
        let strategy = LaunchScalper::new(config);

        let base = Lamports(1_000_000_000); // 1 SOL

        assert_eq!(strategy.calculate_size(base, 0), Lamports(250_000_000)); // Scout
        assert_eq!(strategy.calculate_size(base, 0), Lamports(1_000_000_000)); // Standard
    }

    #[test]
    fn strategy_registry() {
        let mut registry = StrategyRegistry::new();
        registry.register(Box::new(LaunchScalper::new(StrategyConfig::default())));

        assert!(registry.get(StrategyId::LaunchScalper).is_some());
        assert!(registry.get(StrategyId::CurveMomentum).is_none());

        // Test default registry has both
        let default_registry = StrategyRegistry::default();
        assert!(default_registry.get(StrategyId::LaunchScalper).is_some());
        assert!(default_registry.get(StrategyId::CurveMomentum).is_some());
    }
}