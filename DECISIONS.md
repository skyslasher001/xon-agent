# Architecture Decision Records

| ID | Date | Decision | Rationale |
|----|------|----------|-----------|
| ADR-0001 | 2024-01-15 | Rust/tokio single-process hot path | Determinism, latency control, no network hops in decision loop |
| ADR-0002 | 2024-01-15 | Yellowstone gRPC primary, WS fallback | Lowest-latency structured stream; WS for compatibility |
| ADR-0003 | 2024-01-15 | Integer lamports only (u64/u128 checked) | No floating-point money bugs; exact arithmetic |
| ADR-0004 | 2024-02-01 | Fee budget inclusion in P&L | Pending human decision — see HANDOFF.md |
| ADR-0005 | 2024-02-01 | Scout economics (0.25×) | Pending human decision — see HANDOFF.md |
| ADR-0006 | 2024-01-20 | Fail-closed safety verdicts | Any uncertainty → block trade; assume rug |
| ADR-0007 | 2024-01-20 | Ledger as sole P&L source | Single source of truth; no drift between accounting and reality |
| ADR-0008 | 2024-01-25 | Exits before entries on every event | Capital preservation; never add to losing position |
| ADR-0009 | 2024-01-30 | BTreeMap for determinism | Reproducible ordering for tests and audit |
| ADR-0010 | 2024-02-01 | Hard ceilings in code (2/20/5 SOL) | Unbypassable guardrails; compile-time enforcement |

## Adding a Decision
When you deviate from the build prompt or make a significant architectural choice:
1. Add a row to this table
2. Reference it in the commit message
3. If it requires human sign-off, note in HANDOFF.md