# Solana Meme Coin Trading Bot — Engineering Prompt

## Project Vision

Build a **production-grade, sophisticated Solana meme coin trading bot** that operates autonomously with institutional-grade risk management, real-time market analysis, and adaptive strategy execution. This is not a simple script — it's a complete trading infrastructure.

---

## Core Requirements

### 1. Architecture: Event-Driven Microservices
```
┌─────────────────────────────────────────────────────────────────┐
│                        ORCHESTRATOR                              │
│  (Strategy Coordinator, Portfolio Manager, Risk Engine)         │
└─────────────────────────────────────────────────────────────────┘
         │                    │                    │
         ▼                    ▼                    ▼
┌───────────────┐    ┌───────────────┐    ┌───────────────┐
│  MARKET DATA  │    │  EXECUTION    │    │  INTELLIGENCE │
│  INGESTION    │    │  ENGINE       │    │  LAYER        │
│               │    │               │    │               │
│ • WebSocket   │    │ • Jito/Tip    │    │ • Narrative   │
│ • Geyser      │    │ • Priority    │    │ • Social      │
│ • RPC Polling │    │ • MEV Protect │    │ • On-chain    │
│ • Historical  │    │ • Retry/Queue │    │ • ML Signals  │
└───────────────┘    └───────────────┘    └───────────────┘
```

### 2. Trading Strategies (Modular, Composable)
| Strategy | Description | Risk Profile |
|----------|-------------|--------------|
| **Sniper** | Buy on launch/creation (pump.fun, Raydium, Meteora) | High |
| **Momentum** | Ride trending tokens with volume/price confirmation | Medium |
| **Mean Reversion** | Fade extreme moves on low-liquidity pairs | Medium-High |
| **Arbitrage** | Cross-DEX (Raydium ↔ Orca ↔ Meteora) + CEX arb | Low |
| **Copy Trading** | Mirror whale/smart money wallets with delay | Medium |
| **Graduation** | Buy pre-graduation, sell post-graduation pump.fun | High |

### 3. Risk Management (Non-Negotiable)
- **Position sizing**: Kelly criterion + max 2% per trade, 10% portfolio
- **Stop-loss**: Dynamic ATR-based + hard 15% max loss
- **Take-profit**: Tiered (30%/50%/20% at 2x/5x/10x)
- **Daily loss limit**: 5% portfolio → halt trading
- **Correlation limit**: Max 3 positions in same narrative/sector
- **Liquidity floor**: Min $50k TVL, $5k 24h volume
- **Rug detection**: Honeypot check, mint/freeze authority, bundle analysis
- **MEV protection**: Jito bundles, priority fees, commit-reveal

### 4. Data Pipeline (Sub-Second Latency)
```
Solana RPC (multiple) → Geyser plugins → Kafka/Redis Streams
                                    │
                    ┌───────────────┼───────────────┐
                    ▼               ▼               ▼
              Token Parser      Price Engine    Event Detector
              (metadata,        (VWAP, TWAP,    (new pool,
               authorities,      order book,      migration,
               supply, tax)       liquidity)       graduation)
```

### 5. Execution Engine
- **Transaction builder**: Versioned transactions, lookup tables, compute budget
- **Fee management**: Dynamic priority fees (Helius/QuickNode fee API)
- **Bundle submission**: Jito block engine + direct leader forwarding
- **Confirmation**: Optimistic → finalized with retry/rollback
- **Simulation**: Pre-flight simulation on every tx

---

## Technical Stack

| Layer | Technology | Rationale |
|-------|------------|-----------|
| **Language** | Rust (core) + TypeScript (orchestration) | Performance + ergonomics |
| **Runtime** | Tokio + Axum (Rust), Node.js (TS) | Async, high-throughput |
| **Blockchain** | solana-sdk, solana-client, anchor-client | Native Solana support |
| **RPC** | Helius, QuickNode, Triton (redundant) | Geyser, enhanced APIs |
| **Streaming** | Yellowstone Geyser → Kafka (Redpanda) | Sub-slot latency |
| **Database** | PostgreSQL (TimescaleDB) + Redis | Time-series + caching |
| **ML** | Candle (Rust) / PyO3 → Python (scikit-learn/Torch) | Inference in hot path |
| **Monitoring** | Prometheus + Grafana + Loki | Full observability |
| **Deployment** | Docker + Kubernetes (k3s) | Self-hosted, portable |

---

## Key Components Detail

### A. Token Discovery & Filtering
```rust
// New token detection from multiple sources
enum TokenSource {
    PumpFun { bonding_curve: Pubkey },
    Raydium { pool: Pubkey, amm: Pubkey },
    Meteora { dlmm: Pubkey, pool: Pubkey },
    Orca { whirlpool: Pubkey },
    Custom { program: Pubkey, discriminator: [u8; 8] },
}

// Filter pipeline (each stage async, parallelizable)
TokenCandidate → [RugCheck] → [LiquidityCheck] → [HolderAnalysis] → [SocialSignal] → ScoredCandidate
```

### B. Rug Detection Engine
- **Static analysis**: Mint authority, freeze authority, supply mutability
- **Dynamic analysis**: Bundle detection (top 10 holders > 80%), sniper wallets
- **Honeypot test**: Simulate buy/sell on forked state (Anvil/Solana-test-validator)
- **Contract verification**: Check if verified on Solscan, decompiled logic review

### C. Smart Money Tracking
- **Wallet tagging**: Known whales, KOLs, dev wallets, team wallets
- **PnL calculation**: Realized + unrealized per wallet
- **Copy logic**: Delay configurable (1-30 blocks), size scaling, anti-frontrun

### D. Narrative & Social Intelligence
- **Twitter/X API**: Track CA mentions from tiered accounts
- **Telegram/Discord**: Monitor alpha channels (read-only)
- **On-chain**: Token program interactions, new holder velocity
- **NLP**: Classify narrative (AI, DePIN, meme, celebrity, political)

### E. Portfolio & PnL Engine
- Real-time mark-to-market using TWAP/oracle prices
- Tax lot tracking (FIFO/LIFO) for realized PnL
- Performance attribution by strategy, token, timeframe
- Risk metrics: Sharpe, Sortino, max drawdown, VaR

---

## Configuration (TOML)

```toml
# config/trading.toml
[general]
mode = "paper"  # paper | live
dry_run = false
log_level = "info"

[risk]
max_position_pct = 0.02
max_portfolio_pct = 0.10
daily_loss_limit_pct = 0.05
stop_loss_atr_mult = 2.0
hard_stop_pct = 0.15
take_profit_tiers = [{pct=0.30, mult=2.0}, {pct=0.50, mult=5.0}, {pct=0.20, mult=10.0}]
min_liquidity_usd = 50_000
min_volume_24h_usd = 5_000
max_correlated_positions = 3

[execution]
priority_fee_mode = "dynamic"  # fixed | dynamic | jito
max_priority_fee_lamports = 1_000_000
jito_tip_lamports = 100_000
simulate_before_send = true
max_retries = 3
retry_backoff_ms = 500

[strategies.sniper]
enabled = true
max_buy_delay_ms = 500
buy_amount_sol = 0.5
target_programs = ["pump_fun", "raydium_launchpad"]

[strategies.momentum]
enabled = true
lookback_minutes = 15
volume_mult = 3.0
price_change_pct = 20.0

[strategies.arbitrage]
enabled = true
min_profit_bps = 50
max_slippage_bps = 30

[data_sources]
helius_rpc = "https://mainnet.helius-rpc.com/?api-key=${HELIUS_KEY}"
quicknode_rpc = "https://${QN_ENDPOINT}.solana-mainnet.quiknode.pro/${QN_TOKEN}/"
geyser_endpoints = ["grpc://geyser1.helius.xyz:443", "grpc://geyser2.helius.xyz:443"]
```

---

## Development Phases

### Phase 1: Foundation (Week 1-2)
- [ ] Project structure, Cargo workspace, CI/CD
- [ ] Multi-RPC client with failover + health checks
- [ ] Geyser subscriber (Yellowstone) → Kafka
- [ ] Token parser (all major DEXs/launchpads)
- [ ] PostgreSQL schema + TimescaleDB hypertables
- [ ] Basic paper trading engine with simulation

### Phase 2: Core Strategies (Week 3-4)
- [ ] Sniper strategy (pump.fun, Raydium, Meteora)
- [ ] Rug detection engine (static + dynamic)
- [ ] Position management (entry, exit, sizing)
- [ ] Risk engine (limits, correlation, daily halt)
- [ ] Jito bundle integration
- [ ] Paper trading validation (1000+ simulated trades)

### Phase 3: Intelligence Layer (Week 5-6)
- [ ] Smart money indexer (top 10k wallets by PnL)
- [ ] Copy trading with configurable delay/scaling
- [ ] Social signal ingestion (Twitter API v2, Telegram MTProto)
- [ ] Narrative classifier (fine-tuned BERT on Solana tokens)
- [ ] Momentum/mean reversion strategies
- [ ] Backtesting framework (historical replay from Geyser)

### Phase 4: Production Hardening (Week 7-8)
- [ ] Live trading with small capital ($100-500)
- [ ] Comprehensive monitoring (latency, PnL, errors, RPC health)
- [ ] Alerting (PagerDuty/Telegram on drawdown, errors, RPC fail)
- [ ] Disaster recovery (kill switch, position flattening)
- [ ] Audit trail (immutable trade logs, signed receipts)
- [ ] Performance optimization (SIMD, connection pooling, batching)

### Phase 5: Advanced (Ongoing)
- [ ] ML-based entry/exit timing (Candle/Torch)
- [ ] Cross-DEX arbitrage with atomic bundles
- [ ] Options/hedging (Drift, Phoenix)
- [ ] Multi-wallet management (sub-accounts, delegation)
- [ ] Web dashboard (real-time PnL, positions, analytics)

---

## Safety & Compliance

- **No private keys in code**: Hardware wallet (Ledger) + AWS KMS / Vault
- **Encrypted config**: SOPS + age for secrets
- **Audit logging**: Every decision + execution recorded to append-only log
- **Kill switch**: Single RPC call to flatten all positions
- **Rate limiting**: Per-RPC, per-program, global
- **Legal**: Terms of service compliance, no wash trading, no market manipulation

---

## Success Metrics

| Metric | Target |
|--------|--------|
| **Uptime** | 99.9% (excluding planned maintenance) |
| **Latency (event → order)** | < 100ms p99 |
| **Fill rate** | > 95% on simulated paper trades |
| **Rug avoidance** | > 99% (verified on historical rugs) |
| **Sharpe (30d)** | > 2.0 |
| **Max drawdown** | < 15% |
| **Daily PnL volatility** | < 3% portfolio |

---

## Repository Structure

```
solana-meme-bot/
├── Cargo.toml                 # Workspace root
├── docker-compose.yml         # Local dev stack
├── kubernetes/                # K8s manifests
├── config/                    # Config templates (TOML)
├── crates/
│   ├── core/                  # Shared types, errors, config
│   ├── rpc-client/            # Multi-RPC, failover, Geyser
│   ├── token-parser/          # DEX/launchpad parsers
│   ├── rug-detector/          # Static + dynamic analysis
│   ├── execution/             # Tx builder, Jito, simulation
│   ├── risk/                  # Risk engine, limits
│   ├── strategies/            # Strategy trait + implementations
│   ├── portfolio/             # PnL, positions, attribution
│   ├── intelligence/          # Social, narrative, copy trading
│   ├── backtest/              # Historical replay engine
│   └── orchestrator/          # Main coordinator, state machine
├── services/
│   ├── api/                   # Axum REST + WebSocket (dashboard)
│   ├── indexer/               # Geyser → Kafka → TimescaleDB
│   └── monitor/               # Prometheus exporters, alerting
├── scripts/                   # Deployment, migration, utils
├── tests/                     # Integration, property-based
└── docs/                      # Architecture, API, runbooks
```

---

## Immediate Next Steps

1. **Initialize repo**: `cargo new --workspace solana-meme-bot`
2. **Set up CI**: GitHub Actions with cargo test, clippy, audit, docker build
3. **Provision infra**: Helius/QuickNode accounts, Kafka (Redpanda), TimescaleDB
4. **Implement RPC client**: Start with multi-endpoint health-checked client
5. **Build Geyser pipeline**: Yellowstone gRPC → Kafka topic per program
6. **Paper trade**: Run 2 weeks paper before any live capital

---

## Open Questions for You

Before I start implementing, I need your decisions on:

1. **Capital allocation**: What's the starting bankroll? (Affects position sizing, strategy selection)
2. **Risk tolerance**: Aggressive (high freq, high risk) vs Conservative (lower freq, capital preservation)?
3. **Strategy priority**: Which 2-3 strategies to build first? (Sniper + Momentum + Copy Trading recommended)
4. **Infrastructure**: Self-hosted (your server) vs Cloud (AWS/GCP)? Kubernetes vs Docker Compose?
5. **Data costs**: Budget for Helius/QuickNode/Geyser? (~$200-500/mo for production grade)
6. **Legal jurisdiction**: Any regulatory constraints? (US, EU, offshore?)
7. **Team**: Solo or multiple operators? (Affects dashboard, RBAC, audit requirements)

---

*This prompt serves as the engineering specification. Once you confirm the open questions, I'll initialize the repository and begin Phase 1 implementation.*