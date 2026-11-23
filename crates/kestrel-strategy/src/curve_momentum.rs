//! Curve Momentum strategy - tracks bonding curve momentum for newly launched tokens.

use crate::{CoreResult, StrategyConfig, StrategyId, StrategySignal, StrategyState, SizeTier, StrategyAction, Strategy, TokenScore, SignalSource};
use kestrel_safety::{SafetyReport, SafetyVerdict};
use kestrel_scorer::TokenScore as ScorerTokenScore;
use kestrel_state::{StateTracker, PoolState};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use async_trait::async_trait;
use tracing::{debug, info, warn};

/// Price point for momentum tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PricePoint {
    slot: crate::Slot,
    price: f64,
    volume: u64,
}

/// Curve Momentum strategy - tracks bonding curve momentum
pub struct CurveMomentum {
    config: StrategyConfig,
    state: StrategyState,
    price_history: Arc<tokio::sync::RwLock<BTreeMap<crate::Pubkey, VecDeque<PricePoint>>>>,
}

impl CurveMomentum {
    pub fn new(config: StrategyConfig) -> Self {
        Self {
            config,
            state: StrategyState {
                strategy_id: StrategyId::CurveMomentum,
                total_trades: 0,
                winning_trades: 0,
                total_pnl: crate::Lamports(0),
                daily_pnl: crate::Lamports(0),
                open_positions: 0,
                current_exposure: crate::Lamports(0),
                is_paused: false,
                promotion_status: crate::PromotionStatus::Paper,
            },
            price_history: Arc::new(tokio::sync::RwLock::new(BTreeMap::new())),
        }
    }

    fn lookback_slots(&self) -> u64 {
        self.config.custom_params
            .get("lookback_slots")
            .and_then(|v| v.as_u64())
            .unwrap_or(50)
    }

    fn momentum_threshold(&self) -> f64 {
        self.config.custom_params
            .get("momentum_threshold")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.05)
    }

    fn volume_threshold(&self) -> f64 {
        self.config.custom_params
            .get("volume_threshold")
            .and_then(|v| v.as_f64())
            .unwrap_or(10.0)
    }

    fn stop_loss_pct(&self) -> f64 {
        self.config.custom_params
            .get("stop_loss_pct")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.15)
    }

    fn take_profit_pct(&self) -> f64 {
        self.config.custom_params
            .get("take_profit_pct")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.50)
    }

    async fn record_price(&self, mint: crate::Pubkey, pool: &PoolState) {
        let price = if pool.reserve_a.0 > 0 {
            pool.reserve_b.0 as f64 / pool.reserve_a.0 as f64
        } else {
            return;
        };

        let volume = pool.reserve_a.0 + pool.reserve_b.0;

        let point = PricePoint {
            slot: pool.slot,
            price,
            volume,
        };

        let mut history = self.price_history.write().await;
        let points = history.entry(mint).or_insert_with(VecDeque::new);
        points.push_back(point);

        let lookback = self.lookback_slots();
        let current_slot = pool.slot.0;
        while let Some(front) = points.front() {
            if current_slot - front.slot.0 > lookback {
                points.pop_front();
            } else {
                break;
            }
        }
    }

    fn calculate_momentum(&self, points: &VecDeque<PricePoint>) -> f64 {
        if points.len() < 2 {
            return 0.0;
        }

        let first = points.front().unwrap();
        let last = points.back().unwrap();

        if first.price <= 0.0 {
            return 0.0;
        }

        let price_change = (last.price - first.price) / first.price;
        let slots_elapsed = last.slot.0.saturating_sub(first.slot.0) as f64;

        let momentum_per_slot = if slots_elapsed > 0.0 {
            price_change / slots_elapsed
        } else {
            0.0
        };

        let total_volume: u64 = points.iter().map(|p| p.volume).sum();
        let volume_weight = (total_volume as f64).ln().max(1.0);

        (momentum_per_slot * volume_weight).max(0.0).min(1.0)
    }

    fn calculate_acceleration(&self, points: &VecDeque<PricePoint>) -> f64 {
        if points.len() < 3 {
            return 0.0;
        }

        let points_vec: Vec<_> = points.iter().collect();
        let n = points_vec.len();

        let mid = n / 2;
        let recent_slope = if mid < n - 1 {
            let p1 = &points_vec[mid];
            let p2 = &points_vec[n - 1];
            if p2.slot > p1.slot {
                (p2.price - p1.price) / (p2.slot.0 - p1.slot.0) as f64
            } else {
                0.0
            }
        } else {
            0.0
        };

        let older_slope = if mid > 0 {
            let p1 = &points_vec[0];
            let p2 = &points_vec[mid - 1];
            if p2.slot > p1.slot {
                (p2.price - p1.price) / (p2.slot.0 - p1.slot.0) as f64
            } else {
                0.0
            }
        } else {
            0.0
        };

        recent_slope - older_slope
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

    async fn evaluate(&self, score: &crate::TokenScore, state: &crate::StateTracker) -> CoreResult<Vec<StrategySignal>> {
        let mut signals = Vec::new();

        if !self.should_enter(score) {
            return Ok(signals);
        }

        if score.safety.overall_verdict == crate::SafetyVerdict::Block {
            return Ok(signals);
        }

        let pool = state.pools.iter()
            .find(|p| p.value().token_a_mint == score.mint || p.value().token_b_mint == score.mint)
            .map(|p| p.value().clone());

        let Some(pool) = pool else {
            return Ok(signals);
        };

        self.record_price(score.mint, &pool).await;

        let history = self.price_history.read().await;
        let Some(points) = history.get(&score.mint) else {
            return Ok(signals);
        };

        if points.len() < 5 {
            return Ok(signals);
        }

        let momentum = self.calculate_momentum(points);
        let acceleration = self.calculate_acceleration(points);
        let current_price = points.back().unwrap().price;

        let has_position = self.state().open_positions > 0;

        let volume_sol = points.iter().map(|p| p.volume).sum::<u64>() as f64 / 1_000_000_000.0;
        let momentum_threshold = self.momentum_threshold();
        let volume_threshold = self.volume_threshold();

        if !has_position {
            if momentum > momentum_threshold && volume_sol > volume_threshold && acceleration > 0.0 {
                let confirmations = if acceleration > 0.001 { 1 } else { 0 }
                    + if score.components.momentum_score > 0.7 { 1 } else { 0 }
                    + if score.components.volume_score > 0.6 { 1 } else { 0 }
                    + if score.components.liquidity_score > 0.5 { 1 } else { 0 };

                if confirmations >= 2 {
                    let size_tier = if confirmations >= 3 {
                        SizeTier::Conviction
                    } else {
                        self.config.size_tier
                    };

                    signals.push(StrategySignal {
                        strategy_id: StrategyId::CurveMomentum,
                        mint: score.mint,
                        action: StrategyAction::Enter,
                        size_tier,
                        confidence: (0.5 + momentum * 0.5).min(0.95),
                        reasoning: format!(
                            "Curve momentum: momentum={:.3}, accel={:.6}, vol={:.1}SOL, confirmations={}",
                            momentum, acceleration, volume_sol, confirmations
                        ),
                        expiry_slot: score.slot + 100,
                    });
                }
            }
        } else {
            let entry_price = current_price;
            let pnl_pct = (current_price - entry_price) / entry_price;

            if pnl_pct <= -self.stop_loss_pct() {
                signals.push(StrategySignal {
                    strategy_id: StrategyId::CurveMomentum,
                    mint: score.mint,
                    action: StrategyAction::Exit,
                    size_tier: SizeTier::Standard,
                    confidence: 0.95,
                    reasoning: format!("Stop loss hit: {:.1}%", pnl_pct * 100.0),
                    expiry_slot: score.slot + 5,
                });
            } else if pnl_pct >= self.take_profit_pct() {
                signals.push(StrategySignal {
                    strategy_id: StrategyId::CurveMomentum,
                    mint: score.mint,
                    action: StrategyAction::Exit,
                    size_tier: SizeTier::Standard,
                    confidence: 0.9,
                    reasoning: format!("Take profit hit: {:.1}%", pnl_pct * 100.0),
                    expiry_slot: score.slot + 5,
                });
            } else if momentum < 0.01 && acceleration < -0.0001 {
                signals.push(StrategySignal {
                    strategy_id: StrategyId::CurveMomentum,
                    mint: score.mint,
                    action: StrategyAction::Exit,
                    size_tier: SizeTier::Standard,
                    confidence: 0.8,
                    reasoning: format!("Momentum reversal: momentum={:.3}, accel={:.6}", momentum, acceleration),
                    expiry_slot: score.slot + 10,
                });
            } else if momentum < self.momentum_threshold() * 0.5 && acceleration < 0.0 {
                signals.push(StrategySignal {
                    strategy_id: StrategyId::CurveMomentum,
                    mint: score.mint,
                    action: StrategyAction::Exit,
                    size_tier: SizeTier::Standard,
                    confidence: 0.7,
                    reasoning: format!("Momentum decay: {:.3}", momentum),
                    expiry_slot: score.slot + 10,
                });
            }
        }

        Ok(signals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[test]
    fn curve_momentum_calculation() {
        let config = StrategyConfig::default();
        let strategy = CurveMomentum::new(config);

        let mut points = VecDeque::new();
        for i in 0..10 {
            points.push_back(PricePoint {
                slot: crate::Slot(i),
                price: 1.0 + (i as f64 * 0.01),
                volume: 1_000_000_000,
            });
        }

        let momentum = strategy.calculate_momentum(&points);
        assert!(momentum > 0.0);

        let acceleration = strategy.calculate_acceleration(&points);
        assert_eq!(acceleration, 0.0);
    }
}