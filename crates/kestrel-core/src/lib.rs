//! Kestrel core types and shared utilities.
//!
//! This crate contains the fundamental types used across all Kestrel crates:
//! - Lamport-based monetary amounts (u64/u128 checked arithmetic)
//! - Solana addresses, signatures, and slot types
//! - Common error types
//! - Configuration structures

use std::num::NonZeroU64;
use std::str::FromStr;

use anyhow::{Context, Result};
use bs58;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// Lamports — the atomic unit of SOL (1 SOL = 1_000_000_000 lamports).
/// All monetary values in Kestrel use this type to ensure integer-only arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Lamports(pub u64);

impl Lamports {
    /// One SOL in lamports
    pub const ONE_SOL: Self = Self(1_000_000_000);

    /// Create from SOL (floating point, for config only — never for calculation)
    pub fn from_sol(sol: f64) -> Self {
        Self((sol * 1_000_000_000.0).round() as u64)
    }

    /// Convert to SOL for display
    pub fn to_sol(self) -> f64 {
        self.0 as f64 / 1_000_000_000.0
    }

    /// Checked addition
    pub fn checked_add(self, other: Self) -> Option<Self> {
        self.0.checked_add(other.0).map(Self)
    }

    /// Checked subtraction
    pub fn checked_sub(self, other: Self) -> Option<Self> {
        self.0.checked_sub(other.0).map(Self)
    }

    /// Checked multiplication
    pub fn checked_mul(self, other: u64) -> Option<Self> {
        self.0.checked_mul(other).map(Self)
    }

    /// Checked division
    pub fn checked_div(self, other: u64) -> Option<Self> {
        if other == 0 {
            None
        } else {
            Some(Self(self.0 / other))
        }
    }

    /// Saturating addition
    pub fn saturating_add(self, other: Self) -> Self {
        Self(self.0.saturating_add(other.0))
    }

    /// Saturating subtraction
    pub fn saturating_sub(self, other: Self) -> Self {
        Self(self.0.saturating_sub(other.0))
    }
}

impl std::ops::Add for Lamports {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        self.checked_add(other).expect("lamports overflow")
    }
}

impl std::ops::Sub for Lamports {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        self.checked_sub(other).expect("lamports underflow")
    }
}

impl std::fmt::Display for Lamports {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} SOL", self.to_sol())
    }
}

impl Serialize for Lamports {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(self.0)
    }
}

impl<'de> Deserialize<'de> for Lamports {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        u64::deserialize(deserializer).map(Self)
    }
}

/// Solana public key (32 bytes)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pubkey(pub [u8; 32]);

impl Pubkey {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::str::FromStr for Pubkey {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let bytes = bs58::decode(s).into_vec()?;
        if bytes.len() != 32 {
            anyhow::bail!("pubkey must be 32 bytes, got {}", bytes.len());
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }
}

impl std::fmt::Display for Pubkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", bs58::encode(self.0).into_string())
    }
}

impl Serialize for Pubkey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Pubkey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

/// Solana signature (64 bytes)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Signature(pub [u8; 64]);

impl Signature {
    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }
}

impl std::str::FromStr for Signature {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let bytes = bs58::decode(s).into_vec()?;
        if bytes.len() != 64 {
            anyhow::bail!("signature must be 64 bytes, got {}", bytes.len());
        }
        let mut arr = [0u8; 64];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }
}

impl std::fmt::Display for Signature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", bs58::encode(self.0).into_string())
    }
}

/// Slot number
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Slot(pub u64);

/// Core error types
#[derive(Error, Debug)]
pub enum CoreError {
    #[error("arithmetic overflow: {0}")]
    Overflow(String),

    #[error("arithmetic underflow: {0}")]
    Underflow(String),

    #[error("division by zero")]
    DivByZero,

    #[error("invalid pubkey: {0}")]
    InvalidPubkey(String),

    #[error("invalid signature: {0}")]
    InvalidSignature(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Result type alias
pub type CoreResult<T> = Result<T, CoreError>;

/// Checked arithmetic helpers
pub mod checked {
    use super::*;

    pub fn add(a: u64, b: u64) -> CoreResult<u64> {
        a.checked_add(b).ok_or_else(|| CoreError::Overflow(format!("{} + {}", a, b)))
    }

    pub fn sub(a: u64, b: u64) -> CoreResult<u64> {
        a.checked_sub(b).ok_or_else(|| CoreError::Underflow(format!("{} - {}", a, b)))
    }

    pub fn mul(a: u64, b: u64) -> CoreResult<u64> {
        a.checked_mul(b).ok_or_else(|| CoreError::Overflow(format!("{} * {}", a, b)))
    }

    pub fn div(a: u64, b: u64) -> CoreResult<u64> {
        if b == 0 {
            Err(CoreError::DivByZero)
        } else {
            Ok(a / b)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lamports_arithmetic() {
        let a = Lamports(1_000_000_000); // 1 SOL
        let b = Lamports(500_000_000); // 0.5 SOL

        assert_eq!((a + b).0, 1_500_000_000);
        assert_eq!((a - b).0, 500_000_000);
        assert_eq!(a.checked_mul(2), Some(Lamports(2_000_000_000)));
        assert_eq!(a.checked_div(2), Some(Lamports(500_000_000)));
    }

    #[test]
    fn lamports_overflow() {
        let max = Lamports(u64::MAX);
        assert_eq!(max.checked_add(Lamports(1)), None);
    }

    #[test]
    fn pubkey_parse() {
        let pk = "11111111111111111111111111111111".parse::<Pubkey>().unwrap();
        assert_eq!(pk.as_bytes(), &[1u8; 32]);
    }
}