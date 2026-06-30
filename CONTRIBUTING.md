# Contributing

SmartDownloadManager is built sprint-by-sprint against `docs/SPRINT_PLAN.md`.
Before opening a PR:

1. Check `docs/FEATURES.md` — find or add the checkbox your work maps to.
2. Check `docs/ARCHITECTURE.md` — new code should live in the right crate/app
   (engine code is UI-agnostic; protocol code goes in `crates/protocols`, etc.)
3. Every change to `crates/engine` or `crates/protocols` needs tests.
4. Update the relevant `docs/FEATURES.md` checkbox in the same PR that ships it.
5. Run locally before pushing:
   ```bash
   cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
   cargo test --workspace
   pnpm -r lint && pnpm -r test
   ```

## Commit style
Conventional commits preferred: `feat(engine): add segment-stealing allocator`,
`fix(protocols): handle chunked transfer-encoding`, `docs: update sprint 2 status`.

## Branching
`main` is always releasable. Feature work happens on `feature/<short-name>`
branches off `main`, merged via PR once CI is green.
