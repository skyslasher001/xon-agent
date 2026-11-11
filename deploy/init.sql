-- Kestrel Database Initialization
-- Run this on PostgreSQL/TimescaleDB startup

-- Enable TimescaleDB extension
CREATE EXTENSION IF NOT EXISTS timescaledb;

-- Create trades table (hypertable for time-series)
CREATE TABLE IF NOT EXISTS trades (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    timestamp TIMESTAMPTZ NOT NULL,
    slot BIGINT NOT NULL,
    signature TEXT NOT NULL,
    strategy_id TEXT NOT NULL,
    venue TEXT NOT NULL,
    input_mint TEXT NOT NULL,
    output_mint TEXT NOT NULL,
    input_amount BIGINT NOT NULL,
    output_amount BIGINT NOT NULL,
    fee_paid BIGINT NOT NULL,
    pnl_realized BIGINT NOT NULL DEFAULT 0,
    is_entry BOOLEAN NOT NULL
);

-- Convert to hypertable
SELECT create_hypertable('trades', 'timestamp', if_not_exists => TRUE);

-- Create indexes for common queries
CREATE INDEX IF NOT EXISTS idx_trades_strategy_id ON trades (strategy_id);
CREATE INDEX IF NOT EXISTS idx_trades_mint ON trades (input_mint, output_mint);
CREATE INDEX IF NOT EXISTS idx_trades_slot ON trades (slot);
CREATE INDEX IF NOT EXISTS idx_trades_signature ON trades (signature);

-- Create positions table
CREATE TABLE IF NOT EXISTS positions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    mint TEXT NOT NULL,
    strategy_id TEXT NOT NULL,
    entry_price BIGINT NOT NULL,
    quantity BIGINT NOT NULL,
    entry_slot BIGINT NOT NULL,
    entry_signature TEXT NOT NULL,
    current_value BIGINT NOT NULL DEFAULT 0,
    unrealized_pnl BIGINT NOT NULL DEFAULT 0,
    is_open BOOLEAN NOT NULL DEFAULT TRUE,
    exit_price BIGINT,
    exit_signature TEXT,
    exit_slot BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Create indexes for positions
CREATE INDEX IF NOT EXISTS idx_positions_strategy_id ON positions (strategy_id);
CREATE INDEX IF NOT EXISTS idx_positions_mint ON positions (mint);
CREATE INDEX IF NOT EXISTS idx_positions_is_open ON positions (is_open);

-- Create P&L snapshots table (hypertable)
CREATE TABLE IF NOT EXISTS pnl_snapshots (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    timestamp TIMESTAMPTZ NOT NULL,
    total_realized BIGINT NOT NULL,
    total_unrealized BIGINT NOT NULL,
    total_fees BIGINT NOT NULL,
    strategy_allocations JSONB NOT NULL DEFAULT '{}'
);

SELECT create_hypertable('pnl_snapshots', 'timestamp', if_not_exists => TRUE);

-- Create strategy state table
CREATE TABLE IF NOT EXISTS strategy_state (
    strategy_id TEXT PRIMARY KEY,
    total_trades BIGINT NOT NULL DEFAULT 0,
    winning_trades BIGINT NOT NULL DEFAULT 0,
    total_pnl BIGINT NOT NULL DEFAULT 0,
    daily_pnl BIGINT NOT NULL DEFAULT 0,
    open_positions INTEGER NOT NULL DEFAULT 0,
    current_exposure BIGINT NOT NULL DEFAULT 0,
    is_paused BOOLEAN NOT NULL DEFAULT FALSE,
    promotion_status TEXT NOT NULL DEFAULT 'paper',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Create risk state table
CREATE TABLE IF NOT EXISTS risk_state (
    id INTEGER PRIMARY KEY DEFAULT 1,
    current_exposure BIGINT NOT NULL DEFAULT 0,
    daily_pnl BIGINT NOT NULL DEFAULT 0,
    daily_loss BIGINT NOT NULL DEFAULT 0,
    peak_equity BIGINT NOT NULL DEFAULT 0,
    current_drawdown DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    consecutive_losses INTEGER NOT NULL DEFAULT 0,
    last_loss_slot BIGINT,
    cooldown_until BIGINT,
    regime_multiplier DOUBLE PRECISION NOT NULL DEFAULT 1.0,
    kill_switch_activated BOOLEAN NOT NULL DEFAULT FALSE,
    blocked_strategies JSONB NOT NULL DEFAULT '{}',
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Insert default risk state
INSERT INTO risk_state (id) VALUES (1)
ON CONFLICT (id) DO NOTHING;

-- Create function to update updated_at timestamp
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ language 'plpgsql';

-- Create triggers for updated_at
CREATE TRIGGER update_positions_updated_at
    BEFORE UPDATE ON positions
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_strategy_state_updated_at
    BEFORE UPDATE ON strategy_state
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_risk_state_updated_at
    BEFORE UPDATE ON risk_state
    FOR EACH ROW
    EXECUTE FUNCTION update_updated_at_column();

-- Create continuous aggregate for hourly P&L (optional)
CREATE MATERIALIZED VIEW IF NOT EXISTS hourly_pnl
WITH (timescaledb.continuous) AS
SELECT
    time_bucket('1 hour', timestamp) AS hour,
    strategy_id,
    SUM(CASE WHEN is_entry THEN -input_amount ELSE output_amount END) AS net_flow,
    SUM(fee_paid) AS total_fees,
    SUM(pnl_realized) AS realized_pnl,
    COUNT(*) AS trade_count
FROM trades
GROUP BY hour, strategy_id;

-- Add refresh policy (refresh every hour)
SELECT add_continuous_aggregate_policy('hourly_pnl',
    start_offset => INTERVAL '2 hours',
    end_offset => INTERVAL '1 hour',
    schedule_interval => INTERVAL '1 hour');

-- Grant permissions (adjust as needed)
-- GRANT ALL PRIVILEGES ON ALL TABLES IN SCHEMA public TO kestrel;
-- GRANT ALL PRIVILEGES ON ALL SEQUENCES IN SCHEMA public TO kestrel;