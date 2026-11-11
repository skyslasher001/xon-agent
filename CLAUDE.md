# Kestrel — Working Rules for AI Agents

These apply to every session (human or AI) that changes this repository.

## Every change is recorded here
- Commit every change with a clear message, then `git push origin kestrel` in the same session.
  No work stays only on a local machine or in a zip.
- One logical change per commit (a feature, a fix, a doc update).
- Before pushing: `cargo test --workspace` must pass. For UI changes also run `cargo test --workspace -- --test-threads=1`.
- After pushing, check the **Test and build** workflow on GitHub Actions. If it fails, fix and push again.

## Releases
- Finishing a roadmap phase (or a fix users need) = a release.
- Bump the version in `Cargo.toml` and `crates/kestrel-bin/Cargo.toml`,
  add a `CHANGELOG.md` section, commit and push.
- That's it: when a push to `kestrel` carries a version with no GitHub Release yet, the build creates
  release `vX.Y.Z` with notes from `CHANGELOG.md` and attaches artifacts.

## Project map
- `crates/` Rust workspace crates (14 crates).
- `config/` Configuration files.
- `deploy/` Deployment artifacts (Docker Compose).
- `docs/` Documentation.
- `tests/` Integration tests, geometry sweep, byte checks, headless suites.
- `DECISIONS.md` records every deviation from the build prompt; add a row when you make one.

## Key invariants (enforced by tests)
- Money = integer lamports only (u64/u128 checked math)
- No `unwrap`/`expect` outside tests
- Fail-closed safety verdicts
- No lookahead (watermark snapshots, achievable-entry labels)
- Ledger is the sole P&L source
- Exits before entries on every event
- Determinism via `BTreeMap`, pure failure injection
- Hard ceilings in code (2 SOL position, 20 SOL exposure, 5 SOL daily loss)

## Current phase
**Phase 3** — "Paper hands, core complete." Launch Scalper, risk, SimExecutor, portfolio, ledger P&L, `backtest`/`pnl`, and live paper trading in `run` are done. Remaining: indicators + Curve Momentum, regime dial, and 24-hour live paper run. Phases 1–2 await mainnet sign-off.

## Branch
Work on branch `kestrel`; don't set a repo-local git identity.

## Secrets
Never commit keys, seeds, or API keys—use `.env` or encrypted keystore.
Paper mode is default; live trading requires explicit human request.

## Verification
Verify all program IDs, IDLs, and API shapes from primary sources.