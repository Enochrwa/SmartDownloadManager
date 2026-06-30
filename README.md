# SmartDownloadManager

> A universal, cross-platform, fully open-source download manager — combining
> and exceeding the best of IDM, Free Download Manager, JDownloader, xdm,
> aria2, Motrix, yt-dlp, and qBittorrent in a single engine.

[![CI](https://github.com/Enochrwa/SmartDownloadManager/actions/workflows/ci.yml/badge.svg)](https://github.com/Enochrwa/SmartDownloadManager/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

## What is this?

One download engine — multi-connection, resumable, verified, queued — for
**every** protocol that matters: HTTP/HTTPS, FTP/FTPS/SFTP/SCP, WebDAV,
BitTorrent/magnet, Metalink, HLS/DASH streams, and (via `yt-dlp`) thousands of
video sites. Headless by design: the same Rust core powers a desktop app
(Windows/macOS/Linux), a CLI, and a REST/WebSocket API.

Built entirely on free and open-source components — no proprietary SDKs, no
paid APIs. See `docs/LICENSING.md` for the full dependency/license inventory.

## Project status

🚧 **Phase 1 (MVP), Sprint 1** — see [`docs/SPRINT_PLAN.md`](docs/SPRINT_PLAN.md)
for exactly what's in flight and [`docs/FEATURES.md`](docs/FEATURES.md) for the
full feature checklist and what's shipped so far.

## Documentation

| Doc | What's in it |
|---|---|
| [`docs/PRD.md`](docs/PRD.md) | Vision, goals, non-goals, MVP success criteria |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | System design, crate layout, tech choices and why |
| [`docs/TECH_DECISIONS.md`](docs/TECH_DECISIONS.md) | Audit of every technology choice, alternatives rejected, and why |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | The five delivery phases |
| [`docs/SPRINT_PLAN.md`](docs/SPRINT_PLAN.md) | Sprint-by-sprint plan for Phase 1 |
| [`docs/FEATURES.md`](docs/FEATURES.md) | Every feature, checkboxed, phase-tagged |
| [`docs/LICENSING.md`](docs/LICENSING.md) | Third-party license tracking |
| [`CONTRIBUTING.md`](CONTRIBUTING.md) | How to contribute |

## Repository layout

```
apps/desktop/        Tauri + React desktop app
crates/engine/        Headless download orchestrator (the core)
crates/protocols/      HTTP/FTP/SFTP/WebDAV/Metalink/HLS/DASH
crates/torrent/          BitTorrent/magnet support
crates/media/             yt-dlp + FFmpeg integration
crates/storage/            SQLite persistence
crates/scheduler/           Scheduling + post-download actions
crates/api-types/              Shared DTOs (sdmd <-> sdm <-> future SDKs)
crates/server/                  sdmd daemon: REST + WebSocket (axum)
crates/cli/                      sdm CLI: in-process engine driver
crates/xtask/                     cargo xtask: cross-platform dev/release automation
packages/ui/          Shared React component library
packages/common-types/ Shared TypeScript types
extensions/chrome/      Browser extension (Manifest V3)
extensions/firefox/      Browser extension (Firefox-compatible)
docs/                  Planning & architecture docs
```

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the full diagram and
rationale.

## Getting started (dev)

Prerequisites: Rust (stable, pinned via `rust-toolchain.toml`), Node.js 20+, pnpm 9+.

```bash
# Rust workspace
cargo build --workspace
cargo nextest run --workspace   # or `cargo test --workspace`
cargo deny check licenses        # license compliance, see docs/LICENSING.md

# Frontend workspace (Turborepo-orchestrated)
pnpm install
pnpm dev          # runs the desktop app in dev mode (Tauri + Vite)
pnpm lint         # Biome
pnpm test         # Vitest, across all packages

# CLI (in-process, no daemon required)
cargo run -p sdm-cli -- download https://example.com/file.zip

# Daemon (REST + WebSocket, for the browser extension / remote clients)
cargo run -p sdm-server

# Cross-platform dev/release automation
cargo xtask check
```

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md). Every change should map to a
checkbox in [`docs/FEATURES.md`](docs/FEATURES.md) and a sprint in
[`docs/SPRINT_PLAN.md`](docs/SPRINT_PLAN.md).

## License

MIT — see [`LICENSE`](LICENSE). Third-party component licensing notes (FFmpeg,
yt-dlp, etc.) are tracked in [`docs/LICENSING.md`](docs/LICENSING.md).
