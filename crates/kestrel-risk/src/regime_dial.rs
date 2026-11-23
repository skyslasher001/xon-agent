//! Regime Dial - automatic market regime detection and position sizing adjustment.

use crate::{CoreResult, Lamports, Slot};
use crate::{MarketRegime, RiskConfig, MarketDataPoint, RegimeDial};
use kestrel_pnl::PortfolioPnL;
use kestrel_strategy::{StrategyId, SizeTier, StrategySignal};
use anyhow::Context;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

/// Market regime classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarketRegime {
    /// Strong upward trend, low volatility - increase position sizes
    BullTrending,
    /// Strong downward trend, high volatility - reduce position sizes
    BearTrending,
    /// Sideways/range-bound, low volatility - normal sizing
    SidewaysQuiet,
    /// High volatility, no clear trend - reduce position sizes
    VolatileChoppy,
    /// Recovery from bear market - gradual increase
    Recovery,
    /// Unknown/transitioning - conservative sizing
    Unknown,
}

impl MarketRegime {
    /// Get the position size multiplier for this regime
    pub fn multiplier(&self, config: &RiskConfig) -> f64 {
        match self {
            MarketRegime::BullTrending => config.regime_max,
            MarketRegime::BearTrending => config.regime_min,
            MarketRegime::SidewaysQuiet => config.regime_default,
            MarketRegime::VolatileChoppy => (config.regime_min + config.regime_default) / 2.0,
            MarketRegime::Recovery => (config.regime_default + config.regime_max) / 2.0,
            MarketRegime::Unknown => config.regime_min,
        }
    }

    /// Human-readable description
    pub fn description(&self) -> &'static str {
        match self {
            MarketRegime::BullTrending => "Bull market - strong uptrend",
            MarketRegime::BearTrending => "Bear market - strong downtrend",
            MarketRegime::SidewaysQuiet => "Sideways - range bound, low volatility",
            MarketRegime::VolatileChoppy => "Volatile/choppy - high volatility, no trend",
            MarketRegime::Recovery => "Recovery - emerging from bear market",
            MarketRegime::Unknown => "Unknown regime - conservative",
        }
    }
}

/// Market data point for regime detection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketDataPoint {
    pub slot: Slot,
    pub price: f64,      // Portfolio value or price
    pub volume: u64,     // Trading volume
    pub returns: f64,    // Period return
    pub volatility: f64, // Rolling volatility
}

/// Regime Dial - automatic market regime detection and position sizing adjustment
pub struct RegimeDial {
    config: RiskConfig,
    price_history: Arc<RwLock<VecDeque<MarketDataPoint>>>,
    current_regime: Arc<RwLock<MarketRegime>>,
    last_update_slot: Arc<RwLock<Option<Slot>>>,
    regime_history: Arc<RwLock<VecDeque<(Slot, MarketRegime)>>>,
}

impl RegimeDial {
    pub fn new(config: RiskConfig) -> Self {
        Self {
            config,
            price_history: Arc::new(RwLock::new(VecDeque::new())),
            current_regime: Arc::new(RwLock::new(MarketRegime::Unknown)),
            last_update_slot: Arc::new(RwLock::new(None)),
            regime_history: Arc::new(RwLock::new(VecDeque::new())),
        }
    }

    /// Update market data and recalculate regime
    pub async fn update(&self, slot: Slot, price: f64, volume: u64, portfolio_value: f64) {
        let returns = self.calculate_returns(portfolio_value).await;
        let volatility = self.calculate_volatility().await;

        let data_point = MarketDataPoint {
            slot,
            price: portfolio_value,
            volume,
            returns,
            volatility,
        };

        // Add to history
        {
            let mut history = self.price_history.write();
            history.push_back(data_point);

            // Keep only lookback period
            let lookback = self.config.regime_lookback_slots;
            let current_slot = slot.0;
            while let Some(front) = history.front() {
                if current_slot - front.slot.0 > lookback {
                    history.pop_front();
                } else {
                    break;
                }
            }
        }

        // Check if we should update regime
        let should_update = {
            let last_update = self.last_update_slot.read().clone();
            match last_update {
                Some(last) => slot.0.saturating_sub(last.0) >= self.config.regime_update_interval_slots,
                None => true,
            }
        };

        if should_update {
            self.recalculate_regime().await;
            *self.last_update_slot.write() = Some(slot);
        }
    }

    /// Calculate returns from portfolio value history
    async fn calculate_returns(&self, current_value: f64) -> f64 {
        let history = self.price_history.read();
        if let Some(first) = history.front() {
            if first.price > 0.0 {
                return (current_value - first.price) / first.price;
            }
        }
        0.0
    }

    /// Calculate rolling volatility
    async fn calculate_volatility(&self) -> f64 {
        let history = self.price_history.read();
        if history.len() < 2 {
            return 0.0;
        }

        let returns: Vec<f64> = history.iter()
            .skip(1)
            .zip(history.iter())
            .map(|(curr, prev)| {
                if prev.price > 0.0 {
                    (curr.price - prev.price) / prev.price
                } else {
                    0.0
                }
            })
            .collect();

        if returns.is_empty() {
            return 0.0;
        }

        let mean = returns.iter().sum::<f64>() / returns.len() as f64;
        let variance = returns.iter()
            .map(|r| (r - mean).powi(2))
            .sum::<f64>() / returns.len() as f64;

        variance.sqrt()
    }

    /// Recalculate market regime based on current data
    async fn recalculate_regime(&self) {
        let history = self.price_history.read();
        if history.len() < 10 {
            return; // Need minimum data
        }

        let volatility = self.calculate_volatility().await;
        let total_return = self.calculate_total_return().await;
        let trend_strength = self.calculate_trend_strength().await;
        let volume_trend = self.calculate_volume_trend().await;

        let new_regime = self.classify_regime(volatility, total_return, trend_strength, volume_trend);

        // Update regime
        let mut current = self.current_regime.write();
        if *current != new_regime {
            info!("Regime changed: {:?} -> {:?}", *current, new_regime);
            *current = new_regime;

            // Record in history
            let mut history = self.regime_history.write();
            history.push_back(new_regime);
            if history.len() > 100 {
                history.pop_front();
            }
        }
    }

    /// Calculate total return over lookback period
    async fn calculate_total_return(&self) -> f64 {
        let history = self.price_history.read();
        if let (Some(first), Some(last)) = (history.front(), history.back()) {
            if first.price > 0.0 {
                return (last.price - first.price) / first.price;
            }
        }
        0.0
    }

    /// Calculate trend strength (-1 to 1)
    async fn calculate_trend_strength(&self) -> f64 {
        let history = self.price_history.read();
        if history.len() < 3 {
            return 0.0;
        }

        let values: Vec<f64> = history.iter().map(|d| d.price).collect();
        let n = values.len();

        // Linear regression slope
        let x_sum: f64 = (0..n as f64).sum();
        let y_sum: f64 = values.iter().sum();
        let xy_sum: f64 = values.iter().enumerate().map(|(i, &y)| i as f64 * y).sum();
        let x2_sum: f64 = (0..n as f64).map(|x| x * x).sum();

        let n_f = n as f64;
        let slope = (n_f * xy_sum - x_sum * y_sum) / (n_f * x2_sum - x_sum * x_sum);

        // Normalize by mean price
        let mean_price = y_sum / n_f;
        if mean_price > 0.0 {
            (slope / mean_price).clamp(-1.0, 1.0)
        } else {
            0.0
        }
    }

    /// Calculate volume trend
    async fn calculate_volume_trend(&self) -> f64 {
        let history = self.price_history.read();
        if history.len() < 2 {
            return 0.0;
        }

        let recent_volume: u64 = history.iter().rev().take(10).map(|d| d.volume).sum();
        let older_volume: u64 = history.iter().rev().skip(10).take(10).map(|d| d.volume).sum();

        if older_volume > 0 {
            (recent_volume as f64 - older_volume as f64) / older_volume as f64
        } else {
            0.0
        }
    }

    /// Classify market regime based on metrics
    fn classify_regime(&self, volatility: f64, total_return: f64, trend_strength: f64, volume_trend: f64) -> MarketRegime {
        let vol_threshold = self.config.regime_volatility_threshold;
        let trend_threshold = self.config.regime_trend_threshold;

        match (volatility > vol_threshold, total_return > 0.0, trend_strength.abs() > trend_threshold) {
            // High volatility
            (true, _, _) => MarketRegime::VolatileChoppy,

            // Low volatility, clear trend
            (false, true, true) if total_return > 0.05 && trend_strength > 0.0 => MarketRegime::BullTrending,
            (false, false, true) if total_return < -0.05 && trend_strength < 0.0 => MarketRegime::BearTrending,
            (false, true, true) => MarketRegime::Recovery,

            // Low volatility, no clear trend
            (false, _, false) => MarketRegime::SidewaysQuiet,

            // Default
            _ => MarketRegime::Unknown,
        }
    }

    /// Get current regime
    pub fn current_regime(&self) -> MarketRegime {
        *self.current_regime.read()
    }

    /// Get current multiplier
    pub fn current_multiplier(&self) -> f64 {
        let regime = self.current_regime();
        match regime {
            MarketRegime::BullTrending => 1.5,
            MarketRegime::BearTrending => 0.5,
            MarketRegime::SidewaysQuiet => 1.0,
            MarketRegime::VolatileChoppy => 0.75,
            MarketRegime::Recovery => 1.25,
            MarketRegime::Unknown => 0.5,
        }
    }

    /// Get regime description
    pub fn regime_description(&self) -> String {
        self.current_regime().description().to_string()
    }

    /// Get regime history
    pub fn regime_history(&self) -> Vec<MarketRegime> {
        self.regime_history.read().iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kestrel_core::{Lamports, Slot};

    #[test]
    fn regime_multipliers() {
        let config = RiskConfig::default();
        assert_eq!(MarketRegime::BullTrending.multiplier(&config), 1.5);
        assert_eq!(MarketRegime::BearTrending.multiplier(&config), 0.5);
        assert_eq!(MarketRegime::SidewaysQuiet.multiplier(&config), 1.0);
        assert_eq!(MarketRegime::VolatileChoppy.multiplier(&config), 1.0);
        assert_eq!(MarketRegime::Recovery.multiplier(&config), 1.25);
        assert_eq!(MarketRegime::Unknown.multiplier(&config), 0.5);
    }
}