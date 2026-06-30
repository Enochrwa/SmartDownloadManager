# Architecture

## 1. Guiding Principle

**The engine is headless and UI-agnostic.** Everything — desktop app, CLI, REST API,
browser extension backend — talks to the same Rust core through a stable internal
API (and eventually the public REST/WebSocket API). The UI is just one client among
several.

```
                         ┌─────────────────────────────┐
                         │        Clients              │
                         │  Desktop UI (Tauri+React)    │
                         │  CLI (sdm)                   │
                         │  Browser Extensions          │
                         │  3rd-party (REST/WS/SDK)     │
                         └──────────────┬───────────────┘
                                        │ IPC / REST / WS
                         ┌──────────────▼───────────────┐
                         │        crates/api            │
                         │  REST + WebSocket server      │
                         │  (axum)                       │
                         └──────────────┬───────────────┘
                                        │
                         ┌──────────────▼───────────────┐
                         │       crates/engine           │
                         │  Download orchestrator         │
                         │  Queue, scheduler hooks,        │
                         │  segment allocator, retry FSM   │
                         └───┬─────────┬─────────┬───────┘
                             │         │         │
                ┌────────────▼─┐ ┌─────▼─────┐ ┌─▼────────────┐
                │ crates/       │ │ crates/    │ │ crates/      │
                │ protocols     │ │ torrent    │ │ media        │
                │ (HTTP/FTP/    │ │ (libtorrent│ │ (yt-dlp +    │
                │ SFTP/WebDAV/  │ │  via rqbit │ │  FFmpeg      │
                │ Metalink/     │ │  bindings) │ │  subprocess  │
                │ HLS/DASH)     │ │            │ │  wrapper)    │
                └───────────────┘ └────────────┘ └──────────────┘
                             │
                ┌────────────▼───────────────┐
                │       crates/storage         │
                │  SQLite (sqlx) — queue,       │
                │  history, settings, checksums │
                └───────────────────────────────┘
```

## 2. Why this stack

| Concern | Choice | Why |
|---|---|---|
| Core engine language | Rust | Memory safety + performance for segmented I/O, easy cross-compilation, great async (tokio) |
| Desktop shell | Tauri 2 | Native, small binary, Rust backend reuses the engine directly with no IPC overhead vs. Electron |
| Frontend | React + TypeScript | Enock's existing stack/expertise, large ecosystem |
| HTTP/FTP/SFTP | `reqwest`/`hyper`, `suppaftp`, `russh` (all Apache-2.0/MIT) | Pure-Rust, no OpenSSL hard dependency headaches |
| BitTorrent | `librqbit` or `cratetorrent` (MIT) | Pure Rust torrent engines, avoids libtorrent's C++ build complexity, fully OSS |
| Video site extraction | `yt-dlp` (Unlicense), invoked as a managed subprocess with a typed Rust wrapper | Reusing yt-dlp's extractor coverage (thousands of sites) is the only realistic way to match it — reimplementing is out of scope |
| Media remux/merge | `FFmpeg` (LGPL/GPL build, subprocess) | Industry standard, no viable pure-Rust replacement at this maturity |
| Local DB | SQLite via `sqlx` | Zero-config, embedded, battle-tested for queue/history |
| REST/WS API | `axum` (MIT) | Async, type-safe, pairs naturally with tokio |
| CLI | `clap` (MIT) | Standard Rust CLI framework |
| Browser extension | WebExtensions API (Manifest V3), TypeScript | Cross-browser (Chrome/Firefox/Edge/Brave/Opera/Vivaldi all support WebExtensions) |

## 3. Crate/App Layout

```
SmartDownloadManager/
├── apps/
│   └── desktop/              # Tauri + React desktop app
│       ├── src/               # React frontend
│       └── src-tauri/         # Tauri Rust shell, wires engine -> UI
├── crates/
│   ├── engine/                # Orchestrator: queue, scheduler hooks, segment allocator, retry FSM
│   ├── protocols/              # HTTP/FTP/SFTP/WebDAV/Metalink/HLS/DASH implementations
│   ├── torrent/                 # BitTorrent/magnet support
│   ├── media/                    # yt-dlp + FFmpeg subprocess wrappers
│   ├── storage/                   # SQLite persistence layer
│   ├── scheduler/                  # Time-window scheduling, post-download actions
│   └── api/                         # REST + WebSocket server (axum)
├── packages/
│   ├── ui/                    # Shared React component library
│   └── common-types/           # Shared TS types (mirrors Rust API DTOs)
├── extensions/
│   ├── chrome/                # Manifest V3 extension
│   └── firefox/               # Manifest V3 (Firefox-compatible) extension
├── docs/                      # PRD, architecture, sprint plan, etc.
└── scripts/                   # Build/release/dev scripts
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
`LICENSING.md`.
