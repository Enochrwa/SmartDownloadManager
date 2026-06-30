# SmartDownloadManager — Product Requirements Document (PRD)

## 1. Vision

Build a **universal, cross-platform download manager** that combines and exceeds the
best capabilities of Internet Download Manager (IDM), Free Download Manager,
JDownloader, Xtreme Download Manager (xdm), aria2, Motrix, yt-dlp, and qBittorrent —
using only free, open-source components.

SmartDownloadManager should be the default answer to: *"What do I use to download
literally anything, reliably, from anywhere?"*

## 2. Goals

- One engine for HTTP(S), FTP/FTPS/SFTP/SCP, WebDAV, BitTorrent/magnet, Metalink,
  HLS/DASH streaming, and arbitrary websites (via extractor plugins).
- Best-in-class throughput via dynamic multi-connection segmentation.
- Resilience: resume across crashes, reboots, network changes, and weeks-long gaps.
- A real plugin/extension ecosystem (browser extensions, site extractors, themes).
- Fully scriptable: CLI, REST API, WebSocket events, Python/JS SDK.
- 100% open-source dependency stack — no proprietary SDKs, no paid APIs.

## 3. Non-Goals (v1)

- Mobile apps (Android/iOS) — deferred to a later phase.
- Built-in antivirus engine — we integrate with the OS-installed AV instead.
- Proprietary cloud sync — only self-hosted sync (e.g. a small self-hosted sync
  server we build, using open protocols).

## 4. Target Platforms

Windows, macOS, Linux (desktop, Tauri-based) for v1. Android planned for a later
phase (Tauri mobile or React Native, TBD at that phase's spike).

## 5. Feature Inventory

This is the authoritative feature list, grouped exactly as scoped, mapped to the
phase in which each will be delivered. See `SPRINT_PLAN.md` for the sprint-by-sprint
breakdown and `ROADMAP.md` for phase definitions.

| # | Feature Area | Phase |
|---|---|---|
| 1 | Core Download Engine (multi-thread, smart resume, retry, mirrors, verification) | Phase 1 (MVP) |
| 2 | Universal Protocol Support (HTTP/FTP/SFTP/WebDAV/BitTorrent/Metalink/HLS/DASH/RTMP) | Phase 1–3 |
| 3 | Browser Integration (extension, clipboard monitor, drag&drop, batch URLs) | Phase 2 |
| 4 | Video Downloading (yt-dlp + FFmpeg integration) | Phase 2 |
| 5 | Scheduling | Phase 3 |
| 6 | Bandwidth Manager | Phase 1 |
| 7 | Download Queue (priority, nested, categories) | Phase 1 |
| 8 | File Management (auto-organize, extraction, AV integration) | Phase 3 |
| 9 | Advanced Network Features (proxy, DoH, IPv6, Happy Eyeballs) | Phase 2 |
| 10 | Authentication (cookies, tokens, OAuth) | Phase 2 |
| 11 | Security (encrypted history/credentials, sandboxing) | Phase 2 |
| 12 | Automation (CLI, REST API, plugin system, hooks) | Phase 1–4 |
| 13 | Search | Phase 2 |
| 14 | UI/UX (dark mode, docking, mini mode, live graphs) | Phase 1–2 |
| 15 | Analytics | Phase 3 |
| 16 | Cross Platform (Win/macOS/Linux) | Phase 1 |
| 17 | AI Features (offline/local) | Phase 5 |
| 18 | Plugin Marketplace | Phase 4 |
| 19 | Cloud Features (self-hosted sync) | Phase 4 |
| 20 | Developer Features (REST/WS API, SDKs) | Phase 4 |
| 21 | Power User Features (regex, rules engine, macros) | Phase 3 |
| 22 | Accessibility | Phase 3 (continuous) |
| 23 | Recovery Features | Phase 1 |
| 24 | Enterprise Features | Phase 5 |
| 25 | Differentiators (crawler, dependency graphs, OCR, checksum DB) | Phase 5 |

Full unabridged feature checklist (every bullet from the original spec) lives in
`docs/FEATURES.md`, with a checkbox per item and a phase tag.

## 6. Success Criteria for v1 (MVP, end of Phase 1)

- Download any direct HTTP/HTTPS/FTP URL with 1–32 dynamic connections.
- Pause/resume survives app restart and OS reboot.
- Checksum verification (MD5/SHA1/SHA256/SHA512/CRC32) on completed files.
- Queue with priorities and categories.
- Bandwidth limiting (global + per-download).
- Cross-platform desktop app (Win/macOS/Linux) with dark/light UI, live speed graph.
- CLI that can drive the same engine headlessly.
- Local SQLite-backed history/queue persistence with crash recovery.

## 7. Constraints

- Every dependency must be free and open-source (MIT/Apache-2.0/BSD/LGPL/GPL —
  license compatibility tracked in `docs/LICENSING.md`).
- Core engine must be usable headlessly (no UI dependency) so it can power the
  desktop app, CLI, and REST API identically.
