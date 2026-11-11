//! Persistence layer for Kestrel.
//!
//! Provides PostgreSQL/TimescaleDB for transactional data and Parquet for analytics.

use kestrel_core::{CoreResult, Lamports, Pubkey, Signature, Slot};
use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Database connection pool
pub type DbPool = PgPool;

/// Trade record for ledger
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TradeRecord {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub slot: i64,
    pub signature: String,
    pub strategy_id: String,
    pub venue: String,
    pub input_mint: String,
    pub output_mint: String,
    pub input_amount: i64,
    pub output_amount: i64,
    pub fee_paid: i64,
    pub pnl_realized: i64,
    pub is_entry: bool,
}

/// Position record
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct PositionRecord {
    pub id: Uuid,
    pub mint: String,
    pub strategy_id: String,
    pub entry_price: i64,
    pub quantity: i64,
    pub entry_slot: i64,
    pub entry_signature: String,
    pub current_value: i64,
    pub unrealized_pnl: i64,
    pub is_open: bool,
}

/// P&L snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PnLSnapshot {
    pub timestamp: DateTime<Utc>,
    pub total_realized: Lamports,
    pub total_unrealized: Lamports,
    pub total_fees: Lamports,
    pub positions: BTreeMap<String, PositionSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionSnapshot {
    pub mint: String,
    pub quantity: i64,
    pub entry_price: i64,
    pub current_price: i64,
    pub unrealized_pnl: i64,
}

/// Store trait for unified persistence
#[async_trait::async_trait]
pub trait Store: Send + Sync {
    async fn record_trade(&self, trade: &TradeRecord) -> CoreResult<()>;
    async fn record_position(&self, position: &PositionRecord) -> CoreResult<()>;
    async fn update_position(&self, position: &PositionRecord) -> CoreResult<()>;
    async fn close_position(&self, id: Uuid, exit_price: i64, exit_signature: &str) -> CoreResult<()>;
    async fn get_open_positions(&self, strategy_id: &str) -> CoreResult<Vec<PositionRecord>>;
    async fn get_pnl_snapshot(&self, since: DateTime<Utc>) -> CoreResult<PnLSnapshot>;
    async fn health_check(&self) -> CoreResult<bool>;
}

/// PostgreSQL store implementation
pub struct PostgresStore {
    pool: DbPool,
}

impl PostgresStore {
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    pub async fn migrate(&self) -> CoreResult<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Store for PostgresStore {
    async fn record_trade(&self, trade: &TradeRecord) -> CoreResult<()> {
        sqlx::query!(
            r#"
            INSERT INTO trades (id, timestamp, slot, signature, strategy_id, venue, input_mint, output_mint, input_amount, output_amount, fee_paid, pnl_realized, is_entry)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            "#,
            trade.id,
            trade.timestamp,
            trade.slot,
            trade.signature,
            trade.strategy_id,
            trade.venue,
            trade.input_mint,
            trade.output_mint,
            trade.input_amount,
            trade.output_amount,
            trade.fee_paid,
            trade.pnl_realized,
            trade.is_entry
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn record_position(&self, position: &PositionRecord) -> CoreResult<()> {
        sqlx::query!(
            r#"
            INSERT INTO positions (id, mint, strategy_id, entry_price, quantity, entry_slot, entry_signature, current_value, unrealized_pnl, is_open)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            "#,
            position.id,
            position.mint,
            position.strategy_id,
            position.entry_price,
            position.quantity,
            position.entry_slot,
            position.entry_signature,
            position.current_value,
            position.unrealized_pnl,
            position.is_open
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn update_position(&self, position: &PositionRecord) -> CoreResult<()> {
        sqlx::query!(
            r#"
            UPDATE positions SET current_value = $1, unrealized_pnl = $2, is_open = $3 WHERE id = $4
            "#,
            position.current_value,
            position.unrealized_pnl,
            position.is_open,
            position.id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn close_position(&self, id: Uuid, exit_price: i64, exit_signature: &str) -> CoreResult<()> {
        sqlx::query!(
            r#"
            UPDATE positions SET current_value = $1, unrealized_pnl = 0, is_open = false, exit_signature = $2 WHERE id = $3
            "#,
            exit_price,
            exit_signature,
            id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn get_open_positions(&self, strategy_id: &str) -> CoreResult<Vec<PositionRecord>> {
        let positions = sqlx::query_as!(
            PositionRecord,
            r#"
            SELECT id, mint, strategy_id, entry_price, quantity, entry_slot, entry_signature, current_value, unrealized_pnl, is_open
            FROM positions WHERE strategy_id = $1 AND is_open = true
            "#,
            strategy_id
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(positions)
    }

    async fn get_pnl_snapshot(&self, since: DateTime<Utc>) -> CoreResult<PnLSnapshot> {
        // TODO: Implement P&L snapshot query
        Ok(PnLSnapshot {
            timestamp: Utc::now(),
            total_realized: Lamports(0),
            total_unrealized: Lamports(0),
            total_fees: Lamports(0),
            positions: BTreeMap::new(),
        })
    }

    async fn health_check(&self) -> CoreResult<bool> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
        Ok(true)
    }
}

/// Parquet writer for analytics
pub struct ParquetWriter {
    data_dir: std::path::PathBuf,
}

impl ParquetWriter {
    pub fn new(data_dir: std::path::PathBuf) -> Self {
        Self { data_dir }
    }

    pub async fn write_trades(&self, trades: &[TradeRecord]) -> CoreResult<()> {
        // TODO: Implement Parquet writing
        Ok(())
    }

    pub async fn write_positions(&self, positions: &[PositionRecord]) -> CoreResult<()> {
        // TODO: Implement Parquet writing
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trade_record_serialization() {
        let trade = TradeRecord {
            id: Uuid::new_v4(),
            timestamp: Utc::now(),
            slot: 100,
            signature: "test".to_string(),
            strategy_id: "scalper".to_string(),
            venue: "Jupiter".to_string(),
            input_mint: "SOL".to_string(),
            output_mint: "USDC".to_string(),
            input_amount: 1_000_000_000,
            output_amount: 100_000_000,
            fee_paid: 5_000,
            pnl_realized: 0,
            is_entry: true,
        };
        let json = serde_json::to_string(&trade).unwrap();
        assert!(json.contains("scalper"));
    }
}