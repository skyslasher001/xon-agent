# Kestrel — Master Engineering Brief

## Core Philosophy
**Patience, precision, and one clean dive.** The bot assumes every token is a scam until proven otherwise, every RPC lies until confirmed, every tx fails until finalized.

## Non-negotiables
- Keys never in repo
- Paper mode default
- Hard caps in code
- Kill switch always works
- Never fabricate on-chain facts
- Fail closed on data uncertainty
- No market manipulation
- Exits outrank entries
- Money only flows to allowed addresses

## Architecture
Single-process hot path (ingest → decode → state → safety → strategy → risk → execution) over lock-light channels. Event-sourced. Mode-agnostic core (backtest/paper/live share 100% decision code).

## Stack
- Rust/tokio
- Yellowstone gRPC primary
- Jito for landing
- Jupiter + OKX DEX for routing
- OKX Exchange for treasury
- PostgreSQL/TimescaleDB + Parquet
- React SPA cockpit behind Tailscale/CF Access

## Safety: "Assume Rug"
Hard fails block:
- Mint/freeze authority present
- Token-2022 dangerous extensions
- Honeypot simulation fails
- LP not burned
- Creator blacklisted

Soft signals feed 0–100 risk score. Wallet labeler tags every address (dev, sniper, bundler, insider, smart_money, etc.).

## Strategies (plug-ins)
1. **Launch Scalper** (core earner, sub-minute)
2. **Curve Momentum**
3. **Graduation Play**
4. **Smart-Money Follow**
5. **Volume Breakout**

Each has budget, caps, enable flag. Promotion gate: ≥2 weeks paper, ≥150 trades, expectancy >0, profit factor ≥1.5.

## Risk: Calculated Aggression
Three tiers:
- Scout 0.25×
- Standard 1×
- Conviction up to 3× with ≥2 independent confirmations

Moonbags on winners. Regime dial scales base size 0.5–1.5×. Shadow accounting on rejections.

## Execution
- Native venue instructions
- Parallel Jito + staked RPC
- Confirm via gRPC stream
- Idempotent client IDs
- Pre-built warm sells for emergencies

## Phases
0. Skeleton
1. Ingest + safety
2. Strategy + risk
3. Paper trading (CURRENT)
4. Cockpit + control
5. Backtest + replay
6. Live hardening
7. ML scorer, attention signals

Each phase ends with working, tested system. Current target: Kestrel 0.x pre-release.