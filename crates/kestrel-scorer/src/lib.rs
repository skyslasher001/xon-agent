//! Token scoring and signal generation for Kestrel.
//!
//! Combines safety analysis, on-chain metrics, and market signals to produce
//! a composite score for each token launch.

use kestrel_core::{CoreResult, Lamports, Pubkey, Slot};
use kestrel_safety::{SafetyReport, SafetyVerdict, WalletLabel};
use kestrel_state::{StateTracker, PoolState};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use tracing::{debug, info};

/// Score components
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreComponents {
    pub safety_score: f32,        // 0-1 (inverted from risk)
    pub liquidity_score: f32,     // 0-1
    pub momentum_score: f32,      // 0-1
    pub volume_score: f32,        // 0-1
    pub holder_score: f32,        // 0-1
    pub smart_money_score: f32,   // 0-1
}

/// Composite score
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenScore {
    pub mint: Pubkey,
    pub slot: Slot,
    pub composite: f32,           // 0-100
    pub components: ScoreComponents,
    pub safety: SafetyReport,
    pub signals: Vec<Signal>,
}

/// Trading signal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub name: String,
    pub strength: f32,            // -1 to 1
    pub confidence: f32,          // 0-1
    pub source: SignalSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalSource {
    Safety,
    Liquidity,
    Momentum,
    Volume,
    HolderDistribution,
    SmartMoney,
    Social,
}

/// Scorer configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScorerConfig {
    pub safety_weight: f32,
    pub liquidity_weight: f32,
    pub momentum_weight: f32,
    pub volume_weight: f32,
    pub holder_weight: f32,
    pub smart_money_weight: f32,
    pub min_score_for_entry: f32,
    pub safety_block_threshold: f32, // Below this = block
}

impl Default for ScorerConfig {
    fn default() -> Self {
        Self {
            safety_weight: 0.30,
            liquidity_weight: 0.20,
            momentum_weight: 0.20,
            volume_weight: 0.10,
            holder_weight: 0.10,
            smart_money_weight: 0.10,
            min_score_for_entry: 60.0,
            safety_block_threshold: 20.0,
        }
    }
}

/// Main scorer
pub struct Scorer {
    config: ScorerConfig,
    state: Arc<StateTracker>,
}

impl Scorer {
    pub fn new(config: ScorerConfig, state: Arc<StateTracker>) -> Self {
        Self { config, state }
    }

    /// Score a token based on safety report and market data
    pub async fn score(&self, safety: &SafetyReport, pools: &[PoolState]) -> CoreResult<TokenScore> {
        let safety_score = self.safety_score(safety);
        let liquidity_score = self.liquidity_score(pools);
        let momentum_score = self.momentum_score(safety.mint).await?;
        let volume_score = self.volume_score(pools);
        let holder_score = self.holder_score(&safety.wallet_labels);
        let smart_money_score = self.smart_money_score(&safety.wallet_labels);

        let composite = self.config.safety_weight * safety_score
            + self.config.liquidity_weight * liquidity_score
            + self.config.momentum_weight * momentum_score
            + self.config.volume_weight * volume_score
            + self.config.holder_weight * holder_score
            + self.config.smart_money_weight * smart_money_score;

        let signals = self.generate_signals(
            &safety,
            safety_score,
            liquidity_score,
            momentum_score,
            volume_score,
            holder_score,
            smart_money_score,
        );

        Ok(TokenScore {
            mint: safety.mint,
            slot: safety.slot,
            composite: composite * 100.0,
            components: ScoreComponents {
                safety_score,
                liquidity_score,
                momentum_score,
                volume_score,
                holder_score,
                smart_money_score,
            },
            safety: safety.clone(),
            signals,
        })
    }

    fn safety_score(&self, safety: &SafetyReport) -> f32 {
        // Invert risk score (0 risk = 1.0, 100 risk = 0.0)
        1.0 - (safety.risk_score as f32 / 100.0)
    }

    fn liquidity_score(&self, pools: &[PoolState]) -> f32 {
        if pools.is_empty() {
            return 0.0;
        }

        // Score based on total liquidity
        let total_liquidity: u64 = pools.iter()
            .map(|p| p.reserve_a.0 + p.reserve_b.0)
            .sum();

        // Logarithmic scaling: 1M = 0.5, 10M = 0.7, 100M = 0.9
        let score = (total_liquidity as f64).log10() / 10.0;
        score.clamp(0.0, 1.0) as f32
    }

    async fn momentum_score(&self, _mint: &Pubkey) -> CoreResult<f32> {
        // TODO: Calculate price momentum from recent trades
        Ok(0.5)
    }

    fn volume_score(&self, pools: &[PoolState]) -> f32 {
        // TODO: Calculate recent volume
        if pools.is_empty() {
            return 0.0;
        }
        0.5
    }

    fn holder_score(&self, labels: &BTreeMap<Pubkey, WalletLabel>) -> f32 {
        if labels.is_empty() {
            return 0.5;
        }

        let total = labels.len() as f32;
        let smart_money = labels.values().filter(|&&l| l == WalletLabel::SmartMoney).count() as f32;
        let insider = labels.values().filter(|&&l| l == WalletLabel::Insider).count() as f32;
        let dev = labels.values().filter(|&&l| l == WalletLabel::Dev).count() as f32;
        let sniper = labels.values().filter(|&&l| l == WalletLabel::Sniper).count() as f32;

        // Good: smart_money, bad: insider/dev/sniper concentration
        let good = smart_money / total;
        let bad = (insider + dev + sniper) / total;

        (good * 2.0 - bad).clamp(0.0, 1.0)
    }

    fn smart_money_score(&self, labels: &BTreeMap<Pubkey, WalletLabel>) -> f32 {
        let smart_count = labels.values().filter(|&&l| l == WalletLabel::SmartMoney).count();
        (smart_count as f32 * 0.2).clamp(0.0, 1.0)
    }

    fn generate_signals(
        &self,
        safety: &SafetyReport,
        safety_score: f32,
        liquidity_score: f32,
        momentum_score: f32,
        volume_score: f32,
        holder_score: f32,
        smart_money_score: f32,
    ) -> Vec<Signal> {
        let mut signals = Vec::new();

        if safety.overall_verdict == SafetyVerdict::Block {
            signals.push(Signal {
                name: "safety_block".into(),
                strength: -1.0,
                confidence: 1.0,
                source: SignalSource::Safety,
            });
        }

        if safety_score > 0.8 {
            signals.push(Signal {
                name: "safety_clean".into(),
                strength: 0.5,
                confidence: 0.8,
                source: SignalSource::Safety,
            });
        }

        if liquidity_score > 0.7 {
            signals.push(Signal {
                name: "high_liquidity".into(),
                strength: 0.6,
                confidence: 0.7,
                source: SignalSource::Liquidity,
            });
        }

        if momentum_score > 0.7 {
            signals.push(Signal {
                name: "strong_momentum".into(),
                strength: 0.7,
                confidence: 0.6,
                source: SignalSource::Momentum,
            });
        }

        if smart_money_score > 0.5 {
            signals.push(Signal {
                name: "smart_money_present".into(),
                strength: 0.8,
                confidence: 0.7,
                source: SignalSource::SmartMoney,
            });
        }

        signals
    }

    /// Check if token meets entry threshold
    pub fn should_enter(&self, score: &TokenScore) -> bool {
        score.composite >= self.config.min_score_for_entry
            && score.components.safety_score >= self.config.safety_block_threshold / 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kestrel_safety::{SafetyReport, SafetyVerdict, WalletLabel};
    use kestrel_state::PoolState;
    use std::collections::BTreeMap;

    #[test]
    fn scorer_weights_sum_to_one() {
        let config = ScorerConfig::default();
        let sum = config.safety_weight + config.liquidity_weight + config.momentum_weight
            + config.volume_weight + config.holder_weight + config.smart_money_weight;
        assert!((sum - 1.0).abs() < 0.001);
    }

    #[test]
    fn safety_score_inversion() {
        let config = ScorerConfig::default();
        let state = Arc::new(kestrel_state::StateTracker::new());
        let scorer = Scorer::new(config, state);

        let safety = SafetyReport {
            mint: Pubkey::new([1u8; 32]),
            slot: Slot(100),
            overall_verdict: SafetyVerdict::Clean,
            risk_score: 0,
            checks: vec![],
            wallet_labels: BTreeMap::new(),
        };
        assert!((scorer.safety_score(&safety) - 1.0).abs() < 0.001);

        let safety_risky = SafetyReport {
            mint: Pubkey::new([1u8; 32]),
            slot: Slot(100),
            overall_verdict: SafetyVerdict::HighRisk,
            risk_score: 80,
            checks: vec![],
            wallet_labels: BTreeMap::new(),
        };
        assert!((scorer.safety_score(&safety_risky) - 0.2).abs() < 0.001);
    }
}