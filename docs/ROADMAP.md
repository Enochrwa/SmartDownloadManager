# Roadmap

Five phases. Each phase is a set of 2-week sprints (see `SPRINT_PLAN.md` for
Phase 1's sprint-by-sprint breakdown in full detail; later phases are scoped at
the epic level here and will get their own detailed sprint plans as we approach
them).

## Phase 1 — MVP Core Engine (Sprints 1–6, ~12 weeks)
Goal: a working, cross-platform desktop downloader that beats a basic download
manager on HTTP/FTP — multi-connection, resumable, verified, queued, bandwidth
limited — with a CLI and REST API on the same engine.

## Phase 2 — Protocol & Browser Expansion (Sprints 7–12, ~12 weeks)
Goal: BitTorrent/magnet, SFTP/SCP/WebDAV, Metalink, HLS/DASH streaming capture,
yt-dlp/FFmpeg video downloading, browser extensions (Chrome/Firefox/Edge/Brave/
Opera/Vivaldi), proxy/DoH/IPv6 networking, authentication (cookies/tokens/OAuth),
encrypted credential storage, search.

## Phase 3 — Power Features (Sprints 13–18, ~12 weeks)
Goal: scheduling, file management automation, analytics dashboards, rules
engine/macros, accessibility pass, RTMP/MMS/RTSP, SMB/NFS, recovery hardening.

## Phase 4 — Ecosystem (Sprints 19–24, ~12 weeks)
Goal: plugin marketplace, plugin/theme/extension/automation SDKs, Python SDK,
self-hosted sync server, public API stabilization (versioned REST/WS contracts).

## Phase 5 — Intelligence & Enterprise (Sprints 25+, ongoing)
Goal: offline AI features (local models for mirror/thread prediction, content
classification, NL search), enterprise policy/audit/quota management, the
"nice-to-have" differentiators (crawler, dependency graphs, OCR, checksum DB).

## Explicitly deferred
- Mobile (Android/iOS) apps — re-scoped once Phase 2 stabilizes; likely a
  separate React Native or Tauri-mobile app reusing `crates/server`'s REST/WS API.
- Built-in antivirus engine — always OS-AV integration, never a bundled scanner.
