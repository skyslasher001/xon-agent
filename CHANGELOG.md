# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Initial project structure with 14 crates workspace
- Core types (Lamports, Pubkey, Signature, Slot) with checked arithmetic
- Ingestion layer with Yellowstone gRPC and WebSocket fallback
- State tracking with watermark-based snapshots for no-lookahead guarantees
- "Assume Rug" safety analyzer with authority checks, Token-2022 screening, honeypot simulation, bundle detection
- Token scorer with composite scoring and signal generation
- Strategy framework with Launch Scalper (core) and extensible plugin system
- Risk management with tiered sizing (Scout/Standard/Conviction), circuit breakers, hard caps, kill switch
- Portfolio management coordinating positions, P&L, risk, and strategy allocations
- Execution engine with Jito bundle support, parallel RPC landing, warm sells
- Ledger-based P&L with FIFO lot accounting
- CLI with paper/backtest/live modes and safety confirmations

### Changed
- N/A

### Deprecated
- N/A

### Removed
- N/A

### Fixed
- N/A

### Security
- Hard caps enforced in code (2 SOL position, 20 SOL exposure, 5 SOL daily loss)
- Kill switch always works
- Fail-closed safety verdicts
- No `unwrap`/`expect` outside tests
- Integer-only monetary arithmetic
- Paper mode default, live trading double-locked

## [0.1.0] - 2024-XX-XX

### Added
- Initial release