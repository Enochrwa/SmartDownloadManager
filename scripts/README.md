# Scripts

Cross-platform dev/release automation lives in `crates/xtask` (run via
`cargo xtask <command>`) rather than here — see `docs/TECH_DECISIONS.md` §12
for why. This directory is reserved for anything that's genuinely
platform-specific and can't reasonably be Rust (e.g. a `.dmg` notarization
helper), which doesn't exist yet.
