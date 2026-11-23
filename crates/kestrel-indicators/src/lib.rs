//! Technical indicators for Kestrel.
//!
//! Provides common technical indicators for momentum, trend, volatility,
//! and volume analysis. All calculations use integer lamports where possible
//! and fixed-point arithmetic to avoid floating-point errors.

use kestrel_core::{CoreResult, Lamports, Pubkey, Slot};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;
use thiserror::Error;
use tracing::{debug, warn};

/// Indicator calculation errors
#[derive(Error, Debug)]
pub enum IndicatorError {
    #[error("Insufficient data: need at least {0} points, got {1}")]
    InsufficientData(usize, usize),
    #[error("Division by zero in calculation")]
    DivisionByZero,
    #[error("Invalid parameter: {0}")]
    InvalidParameter(String),
    #[error("Price data inconsistent: {0}")]
    InconsistentData(String),
}

/// Price point with timestamp
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricePoint {
    pub slot: Slot,
    pub price: f64,      // Price in SOL per token
    pub volume: u64,     // Volume in lamports
    pub high: f64,       // High price in period
    pub low: f64,        // Low price in period
    pub open: f64,       // Open price
    pub close: f64,      // Close price
}

impl PricePoint {
    pub fn new(slot: Slot, price: f64, volume: u64) -> Self {
        Self {
            slot,
            price,
            volume,
            high: price,
            low: price,
            open: price,
            close: price,
        }
    }

    pub fn from_pool(pool: &crate::kestrel_state::PoolState) -> Option<Self> {
        if pool.reserve_a.0 == 0 {
            return None;
        }
        let price = pool.reserve_b.0 as f64 / pool.reserve_a.0 as f64;
        let volume = pool.reserve_a.0 + pool.reserve_b.0;
        Some(Self::new(pool.slot, price, volume))
    }
}

/// Moving Average type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MAType {
    Simple,      // SMA
    Exponential, // EMA
    Weighted,    // WMA
}

/// Technical indicator result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndicatorValue {
    pub value: f64,
    pub slot: Slot,
    pub signal: Option<Signal>,
}

/// Trading signal from indicator
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Signal {
    Buy,
    Sell,
    Neutral,
}

/// Moving Average calculator
pub struct MovingAverage {
    period: usize,
    ma_type: MAType,
    values: VecDeque<f64>,
    ema_alpha: Option<f64>,
    sum: f64, // For SMA optimization
}

impl MovingAverage {
    pub fn new(period: usize, ma_type: MAType) -> CoreResult<Self> {
        if period < 2 {
            return Err(IndicatorError::InvalidParameter("Period must be at least 2".into()).into());
        }

        let ema_alpha = if ma_type == MAType::Exponential {
            Some(2.0 / (period as f64 + 1.0))
        } else {
            None
        };

        Ok(Self {
            period,
            ma_type,
            values: VecDeque::with_capacity(period),
            ema_alpha,
            sum: 0.0,
        })
    }

    pub fn push(&mut self, value: f64) {
        match self.ma_type {
            MAType::Simple => {
                self.values.push_back(value);
                self.sum += value;
                if self.values.len() > self.period {
                    if let Some(old) = self.values.pop_front() {
                        self.sum -= old;
                    }
                }
            }
            MAType::Exponential => {
                self.values.push_back(value);
                if self.values.len() > self.period {
                    self.values.pop_front();
                }
            }
            MAType::Weighted => {
                self.values.push_back(value);
                if self.values.len() > self.period {
                    self.values.pop_front();
                }
            }
        }
    }

    pub fn value(&self) -> Option<f64> {
        if self.values.len() < self.period {
            return None;
        }

        match self.ma_type {
            MAType::Simple => Some(self.sum / self.period as f64),
            MAType::Exponential => {
                let alpha = self.ema_alpha.unwrap_or(0.0);
                let mut ema = self.values[0];
                for &value in &self.values[1..] {
                    ema = alpha * value + (1.0 - alpha) * ema;
                }
                Some(ema)
            }
            MAType::Weighted => {
                let mut weighted_sum = 0.0;
                let mut weight_sum = 0.0;
                for (i, &value) in self.values.iter().enumerate() {
                    let weight = (i + 1) as f64;
                    weighted_sum += value * weight;
                    weight_sum += weight;
                }
                Some(weighted_sum / weight_sum)
            }
        }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_ready(&self) -> bool {
        self.values.len() >= self.period
    }
}

/// RSI (Relative Strength Index) calculator
pub struct RSI {
    period: usize,
    gains: VecDeque<f64>,
    losses: VecDeque<f64>,
    avg_gain: f64,
    avg_loss: f64,
    last_price: Option<f64>,
}

impl RSI {
    pub fn new(period: usize) -> CoreResult<Self> {
        if period < 2 {
            return Err(IndicatorError::InvalidParameter("RSI period must be at least 2".into()).into());
        }

        Ok(Self {
            period,
            gains: VecDeque::with_capacity(period),
            losses: VecDeque::with_capacity(period),
            avg_gain: 0.0,
            avg_loss: 0.0,
            last_price: None,
        })
    }

    pub fn push(&mut self, price: f64) {
        if let Some(last) = self.last_price {
            let change = price - last;
            let gain = change.max(0.0);
            let loss = (-change).max(0.0);

            self.gains.push_back(gain);
            self.losses.push_back(loss);

            if self.gains.len() > self.period {
                self.gains.pop_front();
                self.losses.pop_front();
            }

            // Calculate average gain/loss (Wilder's smoothing)
            if self.gains.len() == self.period {
                if self.avg_gain == 0.0 && self.avg_loss == 0.0 {
                    // Initial calculation
                    self.avg_gain = self.gains.iter().sum::<f64>() / self.period as f64;
                    self.avg_loss = self.losses.iter().sum::<f64>() / self.period as f64;
                } else {
                    // Wilder's smoothing
                    let gain_sum: f64 = self.gains.iter().sum();
                    let loss_sum: f64 = self.losses.iter().sum();
                    self.avg_gain = (self.avg_gain * (self.period - 1) as f64 + gain_sum) / self.period as f64;
                    self.avg_loss = (self.avg_loss * (self.period - 1) as f64 + loss_sum) / self.period as f64;
                }
            }
        }

        self.last_price = Some(price);
    }

    pub fn value(&self) -> Option<f64> {
        if self.gains.len() < self.period {
            return None;
        }

        if self.avg_loss == 0.0 {
            return Some(100.0);
        }

        let rs = self.avg_gain / self.avg_loss;
        Some(100.0 - (100.0 / (1.0 + rs)))
    }

    pub fn is_ready(&self) -> bool {
        self.gains.len() >= self.period
    }
}

/// MACD (Moving Average Convergence Divergence)
pub struct MACD {
    fast_ema: MovingAverage,
    slow_ema: MovingAverage,
    signal_ema: MovingAverage,
    macd_line: f64,
    signal_line: f64,
    histogram: f64,
}

impl MACD {
    pub fn new(fast_period: usize, slow_period: usize, signal_period: usize) -> CoreResult<Self> {
        if fast_period >= slow_period {
            return Err(IndicatorError::InvalidParameter("Fast period must be less than slow period".into()).into());
        }

        Ok(Self {
            fast_ema: MovingAverage::new(fast_period, MAType::Exponential)?,
            slow_ema: MovingAverage::new(slow_period, MAType::Exponential)?,
            signal_ema: MovingAverage::new(signal_period, MAType::Exponential)?,
            macd_line: 0.0,
            signal_line: 0.0,
            histogram: 0.0,
        })
    }

    pub fn push(&mut self, price: f64) {
        self.fast_ema.push(price);
        self.slow_ema.push(price);

        if self.fast_ema.is_ready() && self.slow_ema.is_ready() {
            self.macd_line = self.fast_ema.value().unwrap() - self.slow_ema.value().unwrap();
            self.signal_ema.push(self.macd_line);

            if self.signal_ema.is_ready() {
                self.signal_line = self.signal_ema.value().unwrap();
                self.histogram = self.macd_line - self.signal_line;
            }
        }
    }

    pub fn macd(&self) -> f64 {
        self.macd_line
    }

    pub fn signal(&self) -> f64 {
        self.signal_line
    }

    pub fn histogram(&self) -> f64 {
        self.histogram
    }

    pub fn is_ready(&self) -> bool {
        self.fast_ema.is_ready() && self.slow_ema.is_ready() && self.signal_ema.is_ready()
    }
}

/// Bollinger Bands
pub struct BollingerBands {
    sma: MovingAverage,
    std_dev_multiplier: f64,
    prices: VecDeque<f64>,
    period: usize,
    upper_band: f64,
    lower_band: f64,
    middle_band: f64,
}

impl BollingerBands {
    pub fn new(period: usize, std_dev_multiplier: f64) -> CoreResult<Self> {
        if period < 2 {
            return Err(IndicatorError::InvalidParameter("Period must be at least 2".into()).into());
        }
        if std_dev_multiplier <= 0.0 {
            return Err(IndicatorError::InvalidParameter("Std dev multiplier must be positive".into()).into());
        }

        Ok(Self {
            sma: MovingAverage::new(period, MAType::Simple)?,
            std_dev_multiplier,
            prices: VecDeque::with_capacity(period),
            period,
            upper_band: 0.0,
            lower_band: 0.0,
            middle_band: 0.0,
        })
    }

    pub fn push(&mut self, price: f64) {
        self.sma.push(price);
        self.prices.push_back(price);

        if self.prices.len() > self.period {
            self.prices.pop_front();
        }

        if self.sma.is_ready() {
            self.middle_band = self.sma.value().unwrap();

            // Calculate standard deviation
            let mean = self.middle_band;
            let variance = self.prices.iter()
                .map(|&p| (p - mean).powi(2))
                .sum::<f64>() / self.period as f64;
            let std_dev = variance.sqrt();

            self.upper_band = mean + self.std_dev_multiplier * std_dev;
            self.lower_band = mean - self.std_dev_multiplier * std_dev;
        }
    }

    pub fn upper(&self) -> f64 {
        self.upper_band
    }

    pub fn lower(&self) -> f64 {
        self.lower_band
    }

    pub fn middle(&self) -> f64 {
        self.middle_band
    }

    pub fn bandwidth(&self) -> f64 {
        if self.middle_band > 0.0 {
            (self.upper_band - self.lower_band) / self.middle_band
        } else {
            0.0
        }
    }

    pub fn is_ready(&self) -> bool {
        self.sma.is_ready()
    }
}

/// Volume Weighted Average Price (VWAP)
pub struct VWAP {
    cumulative_volume: u128,
    cumulative_price_volume: f64,
}

impl VWAP {
    pub fn new() -> Self {
        Self {
            cumulative_volume: 0,
            cumulative_price_volume: 0.0,
        }
    }

    pub fn push(&mut self, price: f64, volume: u64) {
        self.cumulative_volume += volume as u128;
        self.cumulative_price_volume += price * volume as f64;
    }

    pub fn value(&self) -> Option<f64> {
        if self.cumulative_volume == 0 {
            None
        } else {
            Some(self.cumulative_price_volume / self.cumulative_volume as f64)
        }
    }

    pub fn reset(&mut self) {
        self.cumulative_volume = 0;
        self.cumulative_price_volume = 0.0;
    }
}

/// Average True Range (ATR)
pub struct ATR {
    period: usize,
    true_ranges: VecDeque<f64>,
    atr_value: f64,
    last_high: Option<f64>,
    last_low: Option<f64>,
    last_close: Option<f64>,
}

impl ATR {
    pub fn new(period: usize) -> CoreResult<Self> {
        if period < 2 {
            return Err(IndicatorError::InvalidParameter("ATR period must be at least 2".into()).into());
        }

        Ok(Self {
            period,
            true_ranges: VecDeque::with_capacity(period),
            atr_value: 0.0,
            last_high: None,
            last_low: None,
            last_close: None,
        })
    }

    pub fn push(&mut self, high: f64, low: f64, close: f64) {
        let tr = if let (Some(last_high), Some(last_low), Some(last_close)) = (self.last_high, self.last_low, self.last_close) {
            let hl = high - low;
            let hc = (high - last_close).abs();
            let lc = (low - last_close).abs();
            hl.max(hc).max(lc)
        } else {
            high - low // First period, use high-low
        };

        self.true_ranges.push_back(tr);

        if self.true_ranges.len() > self.period {
            self.true_ranges.pop_front();
        }

        if self.true_ranges.len() == self.period {
            self.atr_value = self.true_ranges.iter().sum::<f64>() / self.period as f64;
        }

        self.last_high = Some(high);
        self.last_low = Some(low);
        self.last_close = Some(close);
    }

    pub fn value(&self) -> f64 {
        self.atr_value
    }

    pub fn is_ready(&self) -> bool {
        self.true_ranges.len() >= self.period
    }
}

/// Stochastic Oscillator
pub struct Stochastic {
    k_period: usize,
    d_period: usize,
    highs: VecDeque<f64>,
    lows: VecDeque<f64>,
    closes: VecDeque<f64>,
    k_values: VecDeque<f64>,
    current_k: f64,
    current_d: f64,
}

impl Stochastic {
    pub fn new(k_period: usize, d_period: usize) -> CoreResult<Self> {
        if k_period < 2 || d_period < 2 {
            return Err(IndicatorError::InvalidParameter("Periods must be at least 2".into()).into());
        }

        Ok(Self {
            k_period,
            d_period,
            highs: VecDeque::with_capacity(k_period),
            lows: VecDeque::with_capacity(k_period),
            closes: VecDeque::with_capacity(k_period),
            k_values: VecDeque::with_capacity(d_period),
            current_k: 0.0,
            current_d: 0.0,
        })
    }

    pub fn push(&mut self, high: f64, low: f64, close: f64) {
        self.highs.push_back(high);
        self.lows.push_back(low);
        self.closes.push_back(close);

        if self.highs.len() > self.k_period {
            self.highs.pop_front();
            self.lows.pop_front();
            self.closes.pop_front();
        }

        if self.highs.len() == self.k_period {
            let highest_high = self.highs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            let lowest_low = self.lows.iter().copied().fold(f64::INFINITY, f64::min);

            if highest_high > lowest_low {
                self.current_k = 100.0 * (close - lowest_low) / (highest_high - lowest_low);
            } else {
                self.current_k = 50.0;
            }

            self.k_values.push_back(self.current_k);

            if self.k_values.len() > self.d_period {
                self.k_values.pop_front();
            }

            if self.k_values.len() == self.d_period {
                self.current_d = self.k_values.iter().sum::<f64>() / self.d_period as f64;
            }
        }
    }

    pub fn k(&self) -> f64 {
        self.current_k
    }

    pub fn d(&self) -> f64 {
        self.current_d
    }

    pub fn is_ready(&self) -> bool {
        self.highs.len() >= self.k_period && self.k_values.len() >= self.d_period
    }
}

/// Indicator registry for managing multiple indicators
pub struct IndicatorRegistry {
    sma_short: MovingAverage,
    sma_long: MovingAverage,
    ema_short: MovingAverage,
    ema_long: MovingAverage,
    rsi: RSI,
    macd: MACD,
    bollinger: BollingerBands,
    vwap: VWAP,
    atr: ATR,
    stochastic: Stochastic,
}

impl IndicatorRegistry {
    pub fn new() -> CoreResult<Self> {
        Ok(Self {
            sma_short: MovingAverage::new(10, MAType::Simple)?,
            sma_long: MovingAverage::new(30, MAType::Simple)?,
            ema_short: MovingAverage::new(12, MAType::Exponential)?,
            ema_long: MovingAverage::new(26, MAType::Exponential)?,
            rsi: RSI::new(14)?,
            macd: MACD::new(12, 26, 9)?,
            bollinger: BollingerBands::new(20, 2.0)?,
            vwap: VWAP::new(),
            atr: ATR::new(14)?,
            stochastic: Stochastic::new(14, 3)?,
        })
    }

    pub fn update(&mut self, price: f64, high: f64, low: f64, volume: u64) {
        self.sma_short.push(price);
        self.sma_long.push(price);
        self.ema_short.push(price);
        self.ema_long.push(price);
        self.rsi.push(price);
        self.macd.push(price);
        self.bollinger.push(price);
        self.vwap.push(price, volume);
        self.atr.push(high, low, price);
        self.stochastic.push(high, low, price);
    }

    pub fn signals(&self) -> IndicatorSignals {
        let mut signals = IndicatorSignals::default();

        // RSI signals
        if let Some(rsi) = self.rsi.value() {
            signals.rsi = rsi;
            if rsi > 70.0 {
                signals.rsi_signal = Signal::Sell;
            } else if rsi < 30.0 {
                signals.rsi_signal = Signal::Buy;
            }
        }

        // MACD signals
        if self.macd.is_ready() {
            signals.macd = self.macd.macd();
            signals.macd_signal = self.macd.signal();
            signals.macd_histogram = self.macd.histogram();

            if signals.macd_histogram > 0.0 && signals.macd > signals.macd_signal {
                signals.macd_signal = Signal::Buy;
            } else if signals.macd_histogram < 0.0 && signals.macd < signals.macd_signal {
                signals.macd_signal = Signal::Sell;
            }
        }

        // Bollinger Bands
        if self.bollinger.is_ready() {
            signals.bb_upper = self.bollinger.upper();
            signals.bb_lower = self.bollinger.lower();
            signals.bb_middle = self.bollinger.middle();
            signals.bb_bandwidth = self.bollinger.bandwidth();

            // Price position within bands
            let price = self.sma_short.value().unwrap_or(0.0);
            if price > self.bollinger.upper() {
                signals.bb_signal = Signal::Sell;
            } else if price < self.bollinger.lower() {
                signals.bb_signal = Signal::Buy;
            }
        }

        // VWAP
        signals.vwap = self.vwap.value().unwrap_or(0.0);

        // ATR
        signals.atr = self.atr.value();

        // Stochastic
        if self.stochastic.is_ready() {
            signals.stoch_k = self.stochastic.k();
            signals.stoch_d = self.stochastic.d();

            if self.stochastic.k() > 80.0 && self.stochastic.d() > 80.0 {
                signals.stoch_signal = Signal::Sell;
            } else if self.stochastic.k() < 20.0 && self.stochastic.d() < 20.0 {
                signals.stoch_signal = Signal::Buy;
            }
        }

        // Moving Average crossovers
        if self.sma_short.is_ready() && self.sma_long.is_ready() {
            let sma_short = self.sma_short.value().unwrap();
            let sma_long = self.sma_long.value().unwrap();
            signals.sma_short = sma_short;
            signals.sma_long = sma_long;

            if sma_short > sma_long && sma_short - sma_long > sma_long * 0.001 {
                signals.ma_cross_signal = Signal::Buy;
            } else if sma_short < sma_long && sma_long - sma_short > sma_long * 0.001 {
                signals.ma_cross_signal = Signal::Sell;
            }
        }

        // EMA crossovers
        if self.ema_short.is_ready() && self.ema_long.is_ready() {
            let ema_short = self.ema_short.value().unwrap();
            let ema_long = self.ema_long.value().unwrap();
            signals.ema_short = ema_short;
            signals.ema_long = ema_long;

            if ema_short > ema_long && ema_short - ema_long > ema_long * 0.001 {
                signals.ema_cross_signal = Signal::Buy;
            } else if ema_short < ema_long && ema_long - ema_short > ema_long * 0.001 {
                signals.ema_cross_signal = Signal::Sell;
            }
        }

        signals
    }
}

/// Aggregated indicator signals
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct IndicatorSignals {
    pub rsi: f64,
    pub rsi_signal: Signal,
    pub macd: f64,
    pub macd_signal_line: f64,
    pub macd_histogram: f64,
    pub macd_signal: Signal,
    pub bb_upper: f64,
    pub bb_lower: f64,
    pub bb_middle: f64,
    pub bb_bandwidth: f64,
    pub bb_signal: Signal,
    pub vwap: f64,
    pub atr: f64,
    pub stoch_k: f64,
    pub stoch_d: f64,
    pub stoch_signal: Signal,
    pub sma_short: f64,
    pub sma_long: f64,
    pub ema_short: f64,
    pub ema_long: f64,
    pub ma_cross_signal: Signal,
    pub ema_cross_signal: Signal,
    pub composite_score: f64,
    pub composite_signal: Signal,
}

impl IndicatorSignals {
    pub fn composite(&mut self) {
        let mut score = 0.0;
        let mut count = 0;

        // RSI
        match self.rsi_signal {
            Signal::Buy => { score += 1.0; count += 1; }
            Signal::Sell => { score -= 1.0; count += 1; }
            _ => {}
        }

        // MACD
        match self.macd_signal {
            Signal::Buy => { score += 1.0; count += 1; }
            Signal::Sell => { score -= 1.0; count += 1; }
            _ => {}
        }

        // Bollinger Bands
        match self.bb_signal {
            Signal::Buy => { score += 0.5; count += 1; }
            Signal::Sell => { score -= 0.5; count += 1; }
            _ => {}
        }

        // MA Cross
        match self.ma_cross_signal {
            Signal::Buy => { score += 1.0; count += 1; }
            Signal::Sell => { score -= 1.0; count += 1; }
            _ => {}
        }

        // EMA Cross
        match self.ema_cross_signal {
            Signal::Buy => { score += 1.0; count += 1; }
            Signal::Sell => { score -= 1.0; count += 1; }
            _ => {}
        }

        // Stochastic
        match self.stoch_signal {
            Signal::Buy => { score += 0.5; count += 1; }
            Signal::Sell => { score -= 0.5; count += 1; }
            _ => {}
        }

        if count > 0 {
            self.composite_score = score / count as f64;
            self.composite_signal = if self.composite_score > 0.3 {
                Signal::Buy
            } else if self.composite_score < -0.3 {
                Signal::Sell
            } else {
                Signal::Neutral
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_average_sma() {
        let mut ma = MovingAverage::new(3, MAType::Simple).unwrap();
        ma.push(1.0);
        ma.push(2.0);
        ma.push(3.0);
        assert_eq!(ma.value(), Some(2.0));
        ma.push(4.0);
        assert_eq!(ma.value(), Some(3.0));
    }

    #[test]
    fn moving_average_ema() {
        let mut ma = MovingAverage::new(3, MAType::Exponential).unwrap();
        ma.push(1.0);
        ma.push(2.0);
        ma.push(3.0);
        let val = ma.value();
        assert!(val.is_some());
        assert!(val.unwrap() > 2.0 && val.unwrap() < 3.0);
    }

    #[test]
    fn rsi_calculation() {
        let mut rsi = RSI::new(3).unwrap();
        rsi.push(10.0);
        rsi.push(12.0);
        rsi.push(11.0);
        rsi.push(13.0);
        assert!(rsi.value().is_some());
    }

    #[test]
    fn macd_calculation() {
        let mut macd = MACD::new(3, 5, 2).unwrap();
        for price in [10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0] {
            macd.push(price);
        }
        assert!(macd.is_ready());
    }

    #[test]
    fn bollinger_bands() {
        let mut bb = BollingerBands::new(5, 2.0).unwrap();
        for price in [10.0, 10.5, 10.2, 10.8, 11.0, 10.5, 10.0] {
            bb.push(price);
        }
        assert!(bb.is_ready());
        assert!(bb.upper() > bb.middle());
        assert!(bb.lower() < bb.middle());
    }

    #[test]
    fn vwap_calculation() {
        let mut vwap = VWAP::new();
        vwap.push(10.0, 100);
        vwap.push(11.0, 200);
        let val = vwap.value().unwrap();
        // (10*100 + 11*200) / 300 = 3200/300 = 10.67
        assert!((val - 10.666).abs() < 0.01);
    }

    #[test]
    fn atr_calculation() {
        let mut atr = ATR::new(3).unwrap();
        atr.push(11.0, 9.0, 10.0);
        atr.push(12.0, 10.0, 11.0);
        atr.push(13.0, 11.0, 12.0);
        assert!(atr.is_ready());
        assert!(atr.value() > 0.0);
    }
}