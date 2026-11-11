# Kestrel

A high-performance Solana meme-coin trading system.

**Status:** Pre-release. Phase 0 (skeleton) and Phase 3 (paper trading) complete; Phases 1-2 code complete awaiting mainnet verification. Runs in paper mode by default.

## Quick Start

```bash
cargo build --workspace
./target/debug/kestrel run --mode paper
```

## Architecture

Single-process hot path: ingest → decode → safety → strategy → risk → execution

- **Yellowstone gRPC** primary ingestion with WebSocket fallback
- **"Assume rug" safety analyzer** — authority checks, Token-2022 screening, honeypot simulation, bundle detection
- **Pluggable strategies** — curve momentum, graduation plays, smart-money follow
- **Tiered bet sizing** — scout/standard/conviction with conviction tracker
- **BIP39-derived wallets** with policy enforcement
- **OKX DEX/exchange integration**
- **Ledger-based P&L** with FIFO lots
- **Telegram/Discord notifications**
- **Web cockpit** inspired by Axiom/GMGN

## Risk Controls

- Hard caps (2 SOL position, 20 SOL exposure, 5 SOL daily loss)
- Circuit breakers
- Kill switch that always works
- Jito + staked-RPC parallel landing prioritizes exits over entries

## Crates

| Crate | Purpose |
|-------|---------|
| `kestrel-core` | Core types, constants, shared utilities |
| `kestrel-venues` | Venue integrations (Jupiter, OKX, Pump.fun, etc.) |
| `kestrel-store` | Persistence layer (PostgreSQL, Parquet) |
| `kestrel-ingest` | Yellowstone gRPC / WebSocket ingestion |
| `kestrel-state` | On-chain state tracking |
| `kestrel-safety` | "Assume rug" safety analyzer |
| `kestrel-scorer` | Token scoring and signal generation |
| `kestrel-pnl` | Ledger-based P&L with FIFO lots |
| `kestrel-exec` | Execution engine |
| `kestrel-strategy` | Pluggable strategy framework |
| `kestrel-risk` | Risk management and position sizing |
| `kestrel-portfolio` | Portfolio management |
| `kestrel-engine` | Main trading engine orchestration |
| `kestrel-bin` | CLI binary entry point |

## Configuration

Copy `.env.example` to `.env` and fill in required values:
- RPC endpoints (Yellowstone gRPC, WebSocket, HTTP)
- Wallet seed (BIP39 mnemonic)
- OKX API credentials
- Telegram/Discord webhook URLs
- Database connection strings

## Safety

⚠️ **Warning:** Most meme coins go to zero, and many are built to take your money. This software can lose everything in its wallet. It runs in **paper mode by default**.

Live trading requires explicit confirmation:
- `KESTREL_LIVE_CONFIRM=I_ACCEPT_RISK`
- `LIVE_EXECUTOR_AVAILABLE=true` (currently hardcoded false)

## License

MIT OR Apache-2.0