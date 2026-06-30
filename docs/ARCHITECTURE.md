# Architecture

> See `docs/TECH_DECISIONS.md` for the full rationale behind every choice
> below, including what changed in the post-bootstrap audit (Revision 2).

## 1. Guiding Principle

**The engine is headless and UI-agnostic.** Everything — desktop app, CLI,
`sdmd` daemon, browser extension backend — talks to the same Rust core
through a stable internal API (and, for remote clients, the REST/WebSocket
API exposed by `sdmd`). The UI is just one client among several.

```
                         ┌─────────────────────────────┐
                         │        Clients              │
                         │  Desktop UI (Tauri+React)    │
                         │  sdm CLI (in-process or RPC) │
                         │  Browser Extensions          │
                         │  3rd-party (REST/WS/SDK)     │
                         └──────────────┬───────────────┘
                                        │ in-process (CLI/desktop) or REST/WS
                         ┌──────────────▼───────────────┐
                         │   crates/server (sdmd)        │
                         │  REST + WebSocket daemon       │
                         │  (axum) — for remote clients    │
                         │  crates/api-types: shared DTOs   │
                         └──────────────┬───────────────┘
                                        │
                         ┌──────────────▼───────────────┐
                         │       crates/engine            │
                         │  Download orchestrator          │
                         │  Queue, scheduler hooks,         │
                         │  segment allocator, retry FSM     │
                         └───┬─────────┬─────────┬─────────┘
                             │         │         │
                ┌────────────▼─┐ ┌─────▼─────┐ ┌─▼────────────┐
                │ crates/       │ │ crates/    │ │ crates/      │
                │ protocols     │ │ torrent    │ │ media        │
                │ (HTTP/FTP/    │ │ (librqbit) │ │ (yt-dlp +    │
                │ SFTP/WebDAV/  │ │            │ │  FFmpeg      │
                │ Metalink/     │ │            │ │  subprocess  │
                │ HLS via       │ │            │ │  wrapper)    │
                │ m3u8-rs, DASH │ │            │ │              │
                │ via dash-mpd) │ │            │ │              │
                └───────────────┘ └────────────┘ └──────────────┘
                             │
                ┌────────────▼───────────────┐
                │       crates/storage         │
                │  SQLite (sqlx) — queue,       │
                │  history, settings, checksums │
                └───────────────────────────────┘
```

The desktop app and `sdm` CLI both link `crates/engine` directly (no network
hop for local use). `sdmd` links it too, but additionally exposes it over
REST/WebSocket for the browser extension and any remote/headless client. See
`docs/TECH_DECISIONS.md` §6 for why the daemon and CLI are split into
separate crates instead of one conflated binary.

## 2. Why this stack

| Concern | Choice | Why |
|---|---|---|
| Core engine language | Rust | Memory safety + performance for segmented I/O, easy cross-compilation, great async (tokio) |
| Desktop shell | Tauri 2 | Native, small binary, Rust backend reuses the engine directly with no IPC overhead vs. Electron; validated by `librqbit`'s own reference Tauri app |
| Frontend | React + TypeScript | Matches existing team expertise (Redux/TanStack Query/MUI elsewhere); Zustand added for streaming queue state |
| HTTP/FTP/SFTP | `reqwest`/`hyper`, `suppaftp`, `russh` (all Apache-2.0/MIT) | Pure-Rust, no OpenSSL hard dependency headaches |
| HLS / MPEG-DASH | `m3u8-rs`, `dash-mpd-rs` (MIT) | Pure-Rust manifest parsing, feeds segments into the existing HTTP downloader + FFmpeg remux |
| BitTorrent | `librqbit` (Apache-2.0) | Actively maintained pure-Rust torrent engine; powers `rqbit`'s own CLI/REST/Tauri stack — same shape as this project |
| Video site extraction | `yt-dlp` (Unlicense), invoked as a managed subprocess with a typed Rust wrapper | Reusing yt-dlp's extractor coverage (thousands of sites) is the only realistic way to match it — reimplementing is out of scope |
| Media remux/merge | `FFmpeg` (LGPL/GPL build, subprocess) | Industry standard, no viable pure-Rust replacement at this maturity |
| Local DB | SQLite via `sqlx` | Zero-config, embedded, real SQL for search/analytics (chosen over `redb`, see TECH_DECISIONS §13) |
| REST/WS daemon | `axum` (MIT) in `crates/server` (`sdmd`) | Async, type-safe, pairs naturally with tokio; separated from the CLI (see TECH_DECISIONS §6) |
| CLI | `clap` (MIT) in `crates/cli` (`sdm`) | Standard Rust CLI framework; runs the engine in-process, no daemon required |
| Repo task runner | Turborepo | Caching + dependency-aware task graph across apps/packages (replaces plain `pnpm -r`) |
| JS/TS lint+format | Biome | One fast tool for lint+format instead of ESLint+Prettier |
| Frontend tests | Vitest + React Testing Library | Vite-native, fast, standard pairing |
| Rust test runner (CI) | `cargo-nextest` | Faster, better-isolated CI test runs |
| License compliance | `cargo-deny` | Enforces the MIT/Apache-2.0 allow-list in CI (see `deny.toml`, `docs/LICENSING.md`) |
| Cross-platform automation | `cargo xtask` (`crates/xtask`) | Dev/release scripting that runs identically on Windows/macOS/Linux — no shell-script divergence |
| Browser extension | WebExtensions API (Manifest V3), TypeScript | Cross-browser (Chrome/Firefox/Edge/Brave/Opera/Vivaldi all support WebExtensions) |

## 3. Crate/App Layout

```
SmartDownloadManager/
├── apps/
│   └── desktop/              # Tauri + React desktop app
│       ├── src/               # React frontend (Zustand store, Vitest tests)
│       └── src-tauri/         # Tauri Rust shell, wires engine -> UI directly
├── crates/
│   ├── engine/                # Orchestrator: queue, scheduler hooks, segment allocator, retry FSM
│   ├── protocols/              # HTTP/FTP/SFTP/WebDAV/Metalink/HLS(m3u8-rs)/DASH(dash-mpd-rs)
│   ├── torrent/                 # BitTorrent/magnet support (librqbit)
│   ├── media/                    # yt-dlp + FFmpeg subprocess wrappers
│   ├── storage/                   # SQLite persistence layer
│   ├── scheduler/                  # Time-window scheduling, post-download actions
│   ├── api-types/                   # Shared DTOs between sdmd, sdm-cli, and future SDKs
│   ├── server/                       # sdmd: REST + WebSocket daemon (axum)
│   ├── cli/                           # sdm: CLI, drives engine in-process or talks to sdmd
│   └── xtask/                          # cargo xtask: cross-platform dev/release automation
├── packages/
│   ├── ui/                    # Shared React component library
│   └── common-types/           # Shared TS types (mirrors crates/api-types DTOs)
├── extensions/
│   ├── chrome/                # Manifest V3 extension
│   └── firefox/               # Manifest V3 (Firefox-compatible) extension
├── configs/                   # Shared tsconfig.base.json, biome.json
├── docs/                      # PRD, architecture, sprint plan, tech decisions, etc.
└── scripts/                   # Repo-level docs/notes (actual automation lives in crates/xtask)
```

## 4. Core Engine Concepts (Phase 1)

- **Job**: a single user-requested download (one URL/magnet/playlist entry).
- **Segment**: one connection's byte-range slice of a Job. The allocator can
  re-slice unfinished segments dynamically ("segment stealing") when a faster
  thread finishes early.
- **Retry FSM**: per-segment state machine classifying failures (`Timeout`,
  `DnsFailure`, `TlsFailure`, `ServerBusy`, `RateLimited`, `HttpError(code)`) and
  applying a tailored backoff/retry strategy per class instead of a flat retry.
- **Verifier**: post-download hash check (MD5/SHA1/SHA256/SHA512/CRC32), with
  per-chunk re-download on corruption instead of restarting the whole file.
- **Persistence**: every Job/Segment state transition is journaled to SQLite so a
  Job can resume after crash/reboot/network change — even weeks later — by
  re-validating the partial file against the server (Range + ETag/Last-Modified)
  before resuming.

## 5. Licensing note

See `docs/LICENSING.md`. Because FFmpeg's GPL-enabled builds and yt-dlp are
invoked as external subprocesses (not statically linked), SmartDownloadManager's
own code can remain MIT-licensed while still bundling/recommending GPL tools —
this needs a final legal pass before any binary redistribution, documented in
`LICENSING.md`. `cargo-deny` (see `deny.toml`) now enforces in CI that no
GPL-family crate is ever pulled in as a *linked* Rust dependency.
