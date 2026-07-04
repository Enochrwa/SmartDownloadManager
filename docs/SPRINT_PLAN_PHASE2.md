# Sprint Plan — Phase 2 (Protocol & Browser Expansion)

Six 2-week sprints (Sprints 7–12), continuing directly from `SPRINT_PLAN.md`
(Phase 1, Sprints 1–6, complete as of `main`). Same format as Phase 1: each
sprint lists a goal, scope tied to `FEATURES.md` checkboxes, and a definition
of done. Crate/library choices below are the ones already locked in
`docs/TECH_DECISIONS.md` and pre-declared in the root `Cargo.toml`
(`librqbit`, `m3u8-rs`, `dash-mpd-rs`) — Phase 2 is where they actually get
wired up.

## Phase 1 carryover — closed out in Sprint 7

`docs/FEATURES.md` §2 and `docs/PRD.md` §6 scope FTP/FTPS into Phase 1
("download any direct HTTP/HTTPS/FTP URL"), but `crates/protocols` currently
only implements HTTP/HTTPS (`src/http.rs`) — FTP/FTPS was never built in
Sprints 1–6. Rather than let this sit as silent debt, Sprint 7 below carries
it forward and closes it before layering SFTP/SCP/WebDAV on top in Sprint 8,
since FTP directory-listing and resume semantics are the base case SFTP/SCP's
segment/resume logic will reuse.

---

## Sprint 7 — BitTorrent/Magnet Engine + FTP/FTPS Carryover
**Goal:** The engine gains a second protocol family entirely — BitTorrent —
alongside closing out the Phase 1 FTP/FTPS gap, so `crates/engine`'s Job
model proves it can orchestrate non-HTTP transfers.

Scope:
- `crates/protocols`: FTP/FTPS client via `suppaftp` (as locked in
  `docs/LICENSING.md`) — download, upload, resume (`REST` command), folder
  listing (`LIST`/`MLSD`), explicit/implicit FTPS (TLS)
- `crates/torrent`: wire up `librqbit` behind a `TorrentJob` type that
  satisfies the same `Job`/progress-event contract `crates/engine` already
  uses for HTTP segments, so the engine treats a torrent as just another job
  kind
- Magnet URI parsing + `.torrent` file parsing; DHT bootstrap, peer exchange
  (PEX), and public/private tracker announce (HTTP + UDP trackers)
- Piece verification (SHA1 per-piece hash, per `librqbit`'s built-in
  verification) surfaced through the same `Verifier` events HTTP downloads
  already emit
- Sequential-piece-priority toggle exposed on the Job (full support for
  torrent streaming lands in Phase 3 per `ROADMAP.md`; this sprint only adds
  the priority knob `librqbit` already gives us for free)
- CLI: `sdm download <magnet-uri-or-.torrent-path>` auto-detected by scheme/
  extension, reusing the existing `download` subcommand
- Storage: `jobs` table gains a `job_kind` discriminator (`http`, `ftp`,
  `torrent`) and a `torrent_meta` side table (info hash, piece count, peer
  count) via a new migration

DoD: `sdm download <magnet-uri>` fetches a small, well-seeded public-domain
torrent (e.g. an official Linux ISO's magnet link) end to end with live
progress; an FTP integration test against a local `pyftpdlib`/`vsftpd`
container in CI verifies upload, download, and resume-after-kill; both job
kinds show up correctly typed in `sdm status`/`sdmd`'s job list.

---

## Sprint 8 — SFTP, SCP, WebDAV
**Goal:** Round out "protocols a sysadmin actually uses" — SSH-based
transfer and WebDAV — on the same segmented-download machinery as HTTP.

Scope:
- `crates/protocols`: SFTP client via `russh` + `russh-sftp` (Apache-2.0, per
  `docs/LICENSING.md`) — download/upload, resume via `SSH_FXP_OPEN` offset,
  directory listing
- SCP support layered on the same `russh` session (simpler, non-resumable
  protocol; document this limitation explicitly rather than faking resume)
- WebDAV client (`PROPFIND`/`GET`/`PUT`/`Range`) built on the existing
  `reqwest` stack already used for HTTP, since WebDAV is HTTP-based —
  reuses the Sprint 1–2 range-request and segment-splitting logic almost
  unchanged
- SSH auth: password, private key (with passphrase prompt), and `known_hosts`
  verification (reject-by-default on host key mismatch, with an explicit
  `--accept-new-hostkey` escape hatch for first connection)
- Segment-stealing and multi-connection logic extended to SFTP/WebDAV where
  the server supports it (WebDAV inherits HTTP range support; SFTP gets
  multi-connection via multiple SSH channels on one session)

DoD: integration tests against local SFTP (`OpenSSH` in a CI container) and
WebDAV (`nginx` with the `dav` module) servers cover download, resume, and
directory listing for both; a segmented SFTP download over 4 channels is
verified faster than single-channel baseline, matching the Sprint 2 HTTP DoD
pattern.

---

## Sprint 9 — Metalink, HLS, MPEG-DASH
**Goal:** The engine can consume a manifest (not just a single URL) and
reconstruct a complete file or stream from many small pieces — the
foundation both Metalink mirrors and adaptive-streaming capture share.

Scope:
- Metalink (`.metalink`/`.meta4`, RFC 5854) parser: multiple mirror URLs +
  pre-supplied hashes per file, feeding directly into the mirror-support and
  verification machinery already built in Sprint 4 (no new mirror-failover
  logic needed — Metalink is just a structured way to populate it)
- HLS (`.m3u8`) support via `m3u8-rs`: master + media playlist parsing,
  variant (quality) selection, segment (`.ts`/`fMP4`) download through the
  existing HTTP downloader, live-playlist polling for in-progress streams
- MPEG-DASH (`.mpd`) support via `dash-mpd-rs`: manifest parsing,
  representation/adaptation-set selection, segment template resolution,
  init + media segment download
- Segment concatenation: HLS/DASH segments are joined (not just byte-range
  merged like HTTP segments) — for MPEG-TS this is a straight concatenation;
  for fMP4 this is deferred to Sprint 10's FFmpeg remux step if audio/video
  are in separate representations
- CLI: `sdm download <url-ending-in-.m3u8-or-.mpd-or-.metalink>`
  auto-detected by extension/content-type

DoD: a public test HLS stream (VOD, not live) downloads all variants'
segments and reconstructs a playable file; a DASH manifest with separate
audio/video adaptation sets downloads both; a Metalink file with 3 mirror
URLs and a SHA256 hash downloads via mirror failover and passes verification
— reusing the exact mirror-failover test pattern from Sprint 4.

---

## Sprint 10 — yt-dlp + FFmpeg Video Downloading
**Goal:** Extend reach from "sites with direct manifests" to "thousands of
video sites," matching yt-dlp's extractor coverage without reimplementing it.

Scope:
- `crates/media`: typed Rust wrapper around a managed `yt-dlp` subprocess —
  version-pinned binary, checksum-verified at install/update time (per the
  `docs/LICENSING.md` action item), invoked with `--dump-json` for metadata
  extraction and `-f`/`--merge-output-format` for the actual fetch, output
  parsed as structured events (not scraped stdout) via `--progress-template`
  in a machine-readable format
- FFmpeg subprocess wrapper: automatic audio+video stream merge, subtitle
  embedding/conversion (SRT/ASS/VTT), thumbnail embedding — LGPL-only build
  flags enforced (no `--enable-gpl`) per `docs/LICENSING.md` item 1
- Site metadata surfaced through the Job model: title, thumbnail, duration,
  chapters, available quality/codec list (144p–8K, AV1/VP9/H264/HEVC)
  presented as selectable options before download starts
- Playlist/channel/album/podcast detection: a playlist URL expands into N
  child Jobs under one parent queue entry, reusing the Sprint 5 queue/
  category system
- Livestream detection: routed to yt-dlp's live-from-start / ongoing-capture
  mode rather than treated as a fixed-length Job
- Binary lifecycle: auto-update check for the bundled `yt-dlp` binary
  (sites change frequently; stale extractors are yt-dlp's #1 failure mode),
  gated behind an explicit user-configurable setting (never silent-updates a
  binary without consent)

DoD: downloading a known-stable public-domain video URL (e.g. an official
open-source project's YouTube upload, or a Creative-Commons-licensed test
video) produces a single merged audio+video file with embedded subtitles
when requested; a playlist URL correctly enqueues each video as a separate,
individually-resumable Job; quality selection between two available formats
is verified to fetch the requested format, not just the default.

---

## Sprint 11 — Browser Extensions + Capture UX
**Goal:** SmartDownloadManager becomes reachable from inside the browser,
matching IDM/FDM's core "browser sees a download, offers to grab it" loop.

Scope:
- WebExtensions (Manifest V3) shared codebase in `extensions/`, targeting
  Chrome, Firefox, Edge, Brave, Opera, and Vivaldi (all MV3-compatible;
  Firefox's MV3 support gets a compatibility shim where its API surface
  still differs from Chromium's)
- Extension talks exclusively to `sdmd` over the existing REST/WebSocket API
  (per the daemon/CLI split in `docs/TECH_DECISIONS.md` §6) — no direct
  filesystem or engine access from the browser process
- Download interception: `chrome.downloads.onDeterminingFilename`-style hook
  to redirect qualifying downloads (by size threshold, file-type allowlist,
  or user opt-in per-site) from the browser's native downloader to `sdmd`
- Auto-detect: video/audio/PDF/ZIP link recognition on hover/right-click
  context menu ("Download with SmartDownloadManager")
- Clipboard monitoring: background poll (with an explicit on/off toggle —
  clipboard access is sensitive) that detects a copied downloadable URL and
  shows a native browser notification with a one-click "Download?" action
- Drag & drop: dropping a URL, link element, or selected text containing
  URLs onto the extension's popup/toolbar icon queues it
- Batch URL detection: pasting a block of text with multiple URLs into the
  extension popup extracts and queues all of them, deduplicated against the
  existing-queue/history check from Sprint 4
- Desktop app gains a corresponding "Extension connected" status indicator
  and a first-run pairing flow (`sdmd` issues a local pairing token the
  extension stores)

DoD: with `sdmd` running locally, installing the Chrome extension and
clicking a direct-download link on a test page hands the download to
SmartDownloadManager instead of Chrome's native downloader, visible live in
the desktop app's queue; the same extension package (or its Firefox-manifest
variant) passes a manual smoke test in Firefox; clipboard-copy of a URL
triggers the "Download?" notification within 2 seconds.

---

## Sprint 12 — Advanced Networking, Authentication, Encrypted Credentials, Search
**Goal:** Close out Phase 2 by making the engine usable behind corporate
networks and auth walls, and making a growing download history actually
navigable.

Scope:
- Proxy support: SOCKS4/SOCKS5/HTTP/HTTPS with username/password auth,
  configurable globally, per-category, or per-Job; applied uniformly across
  HTTP, FTP, SFTP (via SSH `ProxyCommand`-equivalent), and torrent
  (`librqbit` proxy passthrough) transports
- DNS over HTTPS (DoH): configurable resolver (default to a well-known
  public DoH endpoint, user-overridable), with plain-DNS fallback on DoH
  failure rather than a hard failure
- IPv4 + IPv6 with Happy Eyeballs (RFC 8305) connection racing; connection
  pooling and TLS session resumption tuned in `reqwest`/`hyper` client
  config to cut repeated-handshake overhead across segments/queue items
- VPN detection: heuristic check (default-route/interface change detection)
  that pauses active downloads and prompts before silently resuming when a
  VPN interface appears/disappears mid-download, since IP-based session
  state (some CDNs, some FTP servers) can otherwise silently corrupt resume
- Authentication: cookie-based sessions (manual cookie paste + browser
  cookie import from the Sprint 11 extension's browser context), bearer
  token / API key headers configurable per-Job or per-domain, OAuth2
  authorization-code flow with local-loopback redirect for sites that
  require it
- Encrypted credential storage: all saved credentials (proxy auth, site
  logins, OAuth refresh tokens) encrypted at rest via OS-native keychains
  (Windows Credential Manager / macOS Keychain / Linux Secret Service via
  `keyring`-style crate) rather than plaintext SQLite columns — the existing
  `storage` schema gains a `credential_ref` indirection instead of storing
  secrets directly
- Search: full-text + filtered search across download history and active
  queue (filename, URL, category, status, date range), regex mode, exposed
  through CLI (`sdm search`), REST (`GET /search`), and a search bar in the
  desktop app, backed by SQLite `FTS5` since Sprint-4-era `storage` is
  already SQLite-based (per `docs/TECH_DECISIONS.md` §13's relational-query
  rationale)

DoD: a download through a configured SOCKS5 proxy with auth succeeds and is
verified (via test proxy server logs) to have actually routed through it; a
cookie-authenticated download against a test site requiring a login session
succeeds where an unauthenticated request would 401; credentials round-trip
through the OS keychain with no secret ever appearing in the SQLite file
(verified by a test that greps the raw DB file for a known test secret and
asserts absence); `sdm search --regex` against a seeded history of 100+ past
downloads returns correct filtered results in under 100ms.

---

## Cross-cutting (every sprint, unchanged from Phase 1)
- Unit + integration tests required for new engine/protocol code (no PR
  merges without tests on `crates/engine`, `crates/protocols`, `crates/torrent`,
  `crates/media`)
- `docs/FEATURES.md` checkboxes updated in the same PR that completes a
  feature — including retroactively checking off any Phase 1 boxes (e.g.
  FTP/FTPS) closed out during Phase 2 cleanup sprints
- CI must be green before merge to `main`, including the new per-protocol
  integration-test containers (FTP/SFTP/WebDAV servers) added to
  `.github/workflows/ci.yml` starting Sprint 7
- Every new dependency checked against `deny.toml`'s license allow-list
  before being added to a `Cargo.toml` (`cargo-deny check licenses` must
  stay green — this is what catches an accidental GPL-linked crate, e.g. if
  a WebDAV or SSH crate turns out to pull one in transitively)
