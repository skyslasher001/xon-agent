# Kestrel Handoff Summary

Kestrel is a **Solana meme-coin trading engine in Rust** that ingests Pump.fun/PumpSwap events, scores launches, and paper-trades a Launch Scalper strategy with a pessimistic simulator and ledger-first P&L.

## Current Status
**Phase 3 core complete (paper trading).** Phases 0–2 code complete; Phases 4–7 not started. **Live trading is double-locked** — requires `KESTREL_LIVE_CONFIRM=I_ACCEPT_RISK` and `LIVE_EXECUTOR_AVAILABLE=true` (currently hardcoded `false`).

## Key Invariants (enforced by tests)
- Money = integer lamports only
- No `unwrap`/`expect` outside tests
- Fail-closed safety verdicts
- No lookahead (watermark snapshots, achievable-entry labels)
- Ledger is the sole P&L source
- Exits before entries on every event
- Determinism via `BTreeMap`, pure failure injection
- Hard ceilings in code (2 SOL position, 20 SOL exposure, 5 SOL daily loss)

## Next Priorities
1. **Phase 1/2 sign-off** — needs RPC endpoint to run mainnet tests and 24h recording
2. **Yellowstone gRPC ingest** — primary source per brief
3. **Finish Phase 3** — indicators crate, Curve Momentum strategy, regime dial, 24h unattended paper run
4. **Human decisions** (ADR 0004): fee budget inclusion, Scout economics
5. **Phase 4** — cockpit + control

## Repo
`github.com/skyslasher001/XON`, branch `kestrel`, PR #1 open. Cloud env pushes branches only; human merges to main.

## Commands
```bash
# Build everything
cargo build --workspace

# Run tests
cargo test --workspace

# Run in paper mode (default)
cargo run --bin kestrel -- run --mode paper

# Run backtest
cargo run --bin kestrel -- backtest --data-path ./data

# Check P&L
cargo run --bin kestrel -- pnl
```

## Running the project
1. Copy `.env.example` to `.env` and configure
2. Ensure Yellowstone gRPC endpoint is available
3. Run `cargo run --bin kestrel -- run --mode paper`

## Key files to understand
- `PROMPT.md` — Master engineering brief
- `DECISIONS.md` — Architecture decision records
- `crates/kestrel-engine/src/lib.rs` — Main engine orchestration
- `crates/kestrel-safety/src/lib.rs` — "Assume rug" safety analyzer
- `crates/kestrel-strategy/src/lib.rs` — Strategy framework
- `crates/kestrel-risk/src/lib.rs` — Risk management