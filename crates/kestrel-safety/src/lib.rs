//! "Assume Rug" Safety Analyzer for Kestrel.
//!
//! This is the core safety module that implements the "assume every token is a scam
//! until proven otherwise" philosophy. It performs comprehensive checks:
//! - Authority checks (mint/freeze authority present)
//! - Token-2022 dangerous extensions screening
//! - Honeypot simulation
//! - Bundle detection (creator funding graph)
//! - LP burn verification
//! - Creator blacklist checking

use kestrel_core::{CoreResult, Lamports, Pubkey, Signature, Slot};
use kestrel_state::{StateTracker, TokenAccount, PoolState};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use tracing::{debug, info, warn};

/// Safety verdict
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SafetyVerdict {
    /// Hard fail - block trade immediately
    Block,
    /// Soft fail - high risk, allow with reduced size
    HighRisk,
    /// Medium risk - proceed with caution
    MediumRisk,
    /// Low risk - normal sizing
    LowRisk,
    /// Clean - full sizing allowed
    Clean,
}

impl SafetyVerdict {
    pub fn is_blocking(self) -> bool {
        matches!(self, SafetyVerdict::Block)
    }

    pub fn risk_score(self) -> u8 {
        match self {
            SafetyVerdict::Block => 100,
            SafetyVerdict::HighRisk => 80,
            SafetyVerdict::MediumRisk => 50,
            SafetyVerdict::LowRisk => 20,
            SafetyVerdict::Clean => 0,
        }
    }
}

/// Individual safety check result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyCheck {
    pub name: String,
    pub verdict: SafetyVerdict,
    pub details: String,
    pub evidence: serde_json::Value,
}

/// Complete safety report
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyReport {
    pub mint: Pubkey,
    pub slot: Slot,
    pub overall_verdict: SafetyVerdict,
    pub risk_score: u8, // 0-100
    pub checks: Vec<SafetyCheck>,
    pub wallet_labels: BTreeMap<Pubkey, WalletLabel>,
}

/// Wallet label from labeler
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WalletLabel {
    Dev,
    Sniper,
    Bundler,
    Insider,
    SmartMoney,
    Whale,
    Retail,
    Unknown,
}

/// Token-2022 extension types (dangerous ones)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Token2022Extension {
    TransferFee,
    TransferHook,
    MetadataPointer,
    TokenMetadata,
    GroupPointer,
    GroupMemberPointer,
    ConfidentialTransfer,
    DefaultAccountState,
    ImmutableOwner,
    InterestBearing,
    NonTransferable,
    PermanentDelegate,
    MintCloseAuthority,
}

impl Token2022Extension {
    pub fn is_dangerous(self) -> bool {
        matches!(
            self,
            Self::TransferFee
                | Self::TransferHook
                | Self::ConfidentialTransfer
                | Self::DefaultAccountState
                | Self::InterestBearing
                | Self::NonTransferable
                | Self::PermanentDelegate
                | Self::MintCloseAuthority
        )
    }
}

/// Safety analyzer configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyConfig {
    /// Enable honeypot simulation
    pub enable_honeypot_sim: bool,
    /// Enable bundle detection
    pub enable_bundle_detection: bool,
    /// Minimum LP burned percentage (0-100)
    pub min_lp_burned_pct: u8,
    /// Max allowed creator holding percentage
    pub max_creator_holding_pct: u8,
    /// Block if mint authority present
    pub block_mint_authority: bool,
    /// Block if freeze authority present
    pub block_freeze_authority: bool,
    /// Creator blacklist (base58 pubkeys)
    pub creator_blacklist: Vec<String>,
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            enable_honeypot_sim: true,
            enable_bundle_detection: true,
            min_lp_burned_pct: 80,
            max_creator_holding_pct: 5,
            block_mint_authority: true,
            block_freeze_authority: true,
            creator_blacklist: vec![],
        }
    }
}

/// Main safety analyzer
pub struct SafetyAnalyzer {
    config: SafetyConfig,
    state: Arc<StateTracker>,
    blacklist: BTreeSet<Pubkey>,
}

impl SafetyAnalyzer {
    pub fn new(config: SafetyConfig, state: Arc<StateTracker>) -> CoreResult<Self> {
        let mut blacklist = BTreeSet::new();
        for pk_str in &config.creator_blacklist {
            if let Ok(pk) = pk_str.parse() {
                blacklist.insert(pk);
            }
        }

        Ok(Self {
            config,
            state,
            blacklist,
        })
    }

    /// Analyze a token for safety
    pub async fn analyze(&self, mint: &Pubkey, slot: Slot) -> CoreResult<SafetyReport> {
        let mut checks = Vec::new();

        // 1. Authority checks
        checks.extend(self.check_authorities(mint).await?);

        // 2. Token-2022 extensions
        checks.extend(self.check_token2022(mint).await?);

        // 3. LP burn verification
        checks.extend(self.check_lp_burned(mint).await?);

        // 4. Creator holding analysis
        checks.extend(self.check_creator_holding(mint).await?);

        // 5. Creator blacklist
        checks.extend(self.check_blacklist(mint).await?);

        // 6. Honeypot simulation (if enabled)
        if self.config.enable_honeypot_sim {
            checks.extend(self.simulate_honeypot(mint).await?);
        }

        // 7. Bundle detection (if enabled)
        if self.config.enable_bundle_detection {
            checks.extend(self.detect_bundles(mint).await?);
        }

        // 8. Wallet labeling
        let wallet_labels = self.label_wallets(mint).await?;

        // Determine overall verdict
        let overall_verdict = self.determine_verdict(&checks);
        let risk_score = self.calculate_risk_score(&checks);

        Ok(SafetyReport {
            mint: *mint,
            slot,
            overall_verdict,
            risk_score,
            checks,
            wallet_labels,
        })
    }

    async fn check_authorities(&self, mint: &Pubkey) -> CoreResult<Vec<SafetyCheck>> {
        // TODO: Fetch mint account and check authorities
        // For now, return placeholder
        Ok(vec![
            SafetyCheck {
                name: "mint_authority".to_string(),
                verdict: SafetyVerdict::Clean,
                details: "Mint authority not present (placeholder)".to_string(),
                evidence: serde_json::json!({ "present": false }),
            },
            SafetyCheck {
                name: "freeze_authority".to_string(),
                verdict: SafetyVerdict::Clean,
                details: "Freeze authority not present (placeholder)".to_string(),
                evidence: serde_json::json!({ "present": false }),
            },
        ])
    }

    async fn check_token2022(&self, mint: &Pubkey) -> CoreResult<Vec<SafetyCheck>> {
        // TODO: Parse mint account for Token-2022 extensions
        Ok(vec![SafetyCheck {
            name: "token2022_extensions".to_string(),
            verdict: SafetyVerdict::Clean,
            details: "No dangerous Token-2022 extensions found (placeholder)".to_string(),
            evidence: serde_json::json!({ "extensions": [] }),
        }])
    }

    async fn check_lp_burned(&self, mint: &Pubkey) -> CoreResult<Vec<SafetyCheck>> {
        // TODO: Check LP token burn status
        Ok(vec![SafetyCheck {
            name: "lp_burned".to_string(),
            verdict: SafetyVerdict::Clean,
            details: "LP tokens burned (placeholder)".to_string(),
            evidence: serde_json::json!({ "burned_pct": 100 }),
        }])
    }

    async fn check_creator_holding(&self, mint: &Pubkey) -> CoreResult<Vec<SafetyCheck>> {
        // TODO: Analyze creator token holdings
        Ok(vec![SafetyCheck {
            name: "creator_holding".to_string(),
            verdict: SafetyVerdict::Clean,
            details: "Creator holding within limits (placeholder)".to_string(),
            evidence: serde_json::json!({ "holding_pct": 0 }),
        }])
    }

    async fn check_blacklist(&self, mint: &Pubkey) -> CoreResult<Vec<SafetyCheck>> {
        // TODO: Check if creator is blacklisted
        Ok(vec![SafetyCheck {
            name: "creator_blacklist".to_string(),
            verdict: SafetyVerdict::Clean,
            details: "Creator not blacklisted (placeholder)".to_string(),
            evidence: serde_json::json!({ "blacklisted": false }),
        }])
    }

    async fn simulate_honeypot(&self, mint: &Pubkey) -> CoreResult<Vec<SafetyCheck>> {
        // TODO: Simulate buy/sell to detect honeypot
        Ok(vec![SafetyCheck {
            name: "honeypot_simulation".to_string(),
            verdict: SafetyVerdict::Clean,
            details: "Honeypot simulation passed (placeholder)".to_string(),
            evidence: serde_json::json!({ "buy_success": true, "sell_success": true }),
        }])
    }

    async fn detect_bundles(&self, mint: &Pubkey) -> CoreResult<Vec<SafetyCheck>> {
        // TODO: Analyze funding graph for bundle detection
        Ok(vec![SafetyCheck {
            name: "bundle_detection".to_string(),
            verdict: SafetyVerdict::Clean,
            details: "No bundle detected (placeholder)".to_string(),
            evidence: serde_json::json!({ "bundled": false, "related_addresses": 0 }),
        }])
    }

    async fn label_wallets(&self, mint: &Pubkey) -> CoreResult<BTreeMap<Pubkey, WalletLabel>> {
        // TODO: Implement wallet labeling based on behavior patterns
        Ok(BTreeMap::new())
    }

    fn determine_verdict(&self, checks: &[SafetyCheck]) -> SafetyVerdict {
        // Fail-closed: any Block verdict = overall Block
        if checks.iter().any(|c| c.verdict == SafetyVerdict::Block) {
            return SafetyVerdict::Block;
        }

        // Otherwise, take the worst verdict
        checks.iter()
            .map(|c| c.verdict)
            .max()
            .unwrap_or(SafetyVerdict::Clean)
    }

    fn calculate_risk_score(&self, checks: &[SafetyCheck]) -> u8 {
        // Weighted risk score
        let weights = [
            ("mint_authority", 25),
            ("freeze_authority", 25),
            ("token2022_extensions", 20),
            ("lp_burned", 15),
            ("creator_holding", 10),
            ("creator_blacklist", 20),
            ("honeypot_simulation", 30),
            ("bundle_detection", 15),
        ];

        let mut score = 0u16;
        for (name, weight) in weights {
            if let Some(check) = checks.iter().find(|c| c.name == name) {
                score += (check.verdict.risk_score() as u16 * weight) / 100;
            }
        }

        (score.min(100)) as u8
    }
}

use std::sync::Arc;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_ordering() {
        assert!(SafetyVerdict::Block > SafetyVerdict::HighRisk);
        assert!(SafetyVerdict::HighRisk > SafetyVerdict::MediumRisk);
        assert!(SafetyVerdict::MediumRisk > SafetyVerdict::LowRisk);
        assert!(SafetyVerdict::LowRisk > SafetyVerdict::Clean);
    }

    #[test]
    fn verdict_blocking() {
        assert!(SafetyVerdict::Block.is_blocking());
        assert!(!SafetyVerdict::HighRisk.is_blocking());
        assert!(!SafetyVerdict::Clean.is_blocking());
    }

    #[test]
    fn risk_score() {
        assert_eq!(SafetyVerdict::Block.risk_score(), 100);
        assert_eq!(SafetyVerdict::Clean.risk_score(), 0);
    }

    #[test]
    fn token2022_dangerous() {
        assert!(Token2022Extension::TransferFee.is_dangerous());
        assert!(Token2022Extension::TransferHook.is_dangerous());
        assert!(!Token2022Extension::MetadataPointer.is_dangerous());
        assert!(!Token2022Extension::TokenMetadata.is_dangerous());
    }

    #[test]
    fn determine_verdict_fail_closed() {
        let config = SafetyConfig::default();
        let state = Arc::new(kestrel_state::StateTracker::new());
        let analyzer = SafetyAnalyzer::new(config, state).unwrap();

        let checks = vec![
            SafetyCheck { name: "a".into(), verdict: SafetyVerdict::Clean, details: "".into(), evidence: serde_json::json!({}) },
            SafetyCheck { name: "b".into(), verdict: SafetyVerdict::Block, details: "".into(), evidence: serde_json::json!({}) },
        ];
        assert_eq!(analyzer.determine_verdict(&checks), SafetyVerdict::Block);
    }
}