# Sprint Plan — Phase 1 (MVP)

Six 2-week sprints. Each sprint lists: goal, scope (tied to `FEATURES.md`
checkboxes), and a definition of done.

---

## Sprint 1 — Project Foundation & Single-Connection HTTP Download
**Goal:** Repo, tooling, and a working single-threaded HTTP/HTTPS downloader end to end (CLI only).

Scope:
- Monorepo scaffolding: Cargo workspace (`crates/*`), pnpm workspace (`apps/*`, `packages/*`)
- CI: lint + build + test on push (GitHub Actions, Linux/macOS/Windows matrix)
- `crates/storage`: SQLite schema for `jobs`, `segments`, `settings` + migrations (sqlx)
- `crates/protocols`: HTTP/HTTPS GET with streaming write-to-disk, TLS 1.2/1.3 via `rustls`
- `crates/engine`: minimal `Job` model, single-segment download, progress events (tokio mpsc channel)
- `crates/api` (`sdm` binary): CLI `download <url> [-o output]` that drives the engine headlessly

DoD: `sdm download https://example.com/file.zip` downloads the file with a live progress bar, writes a row to SQLite, and exits 0.

---

## Sprint 2 — Multi-Connection Segmented Downloading
**Goal:** Real concurrent-connection downloading with dynamic segment allocation.

Scope:
- Range-request probing (does server support `Accept-Ranges`?)
- Segment splitting: 1–128 connections, adaptive chunk size based on file size
- Segment-stealing allocator (finished thread picks up unfinished work from the slowest segment)
- Per-server connection limit config
- Chunk merge-on-completion (write segments to a single pre-allocated file via positioned writes, no separate merge pass)
- CLI flag `--connections N` / `--connections auto`

DoD: a 500MB+ test file downloads with 8+ concurrent connections, verified faster than single-connection baseline; killing one connection mid-download doesn't corrupt the file.

---

## Sprint 3 — Smart Resume & Intelligent Retry
**Goal:** Downloads survive crashes, reboots, and network changes; failures are classified and retried intelligently.

Scope:
- Segment-state journaling to SQLite on every state transition (so a crash mid-download is fully recoverable)
- Resume validation on restart: re-check `ETag`/`Last-Modified`/`Content-Length` before resuming a partial file; fall back to restart if the server-side resource changed
- Retry FSM: classify `Timeout`, `DnsFailure`, `TlsFailure`, `ServerBusy (429/503)`, `HttpError(code)`; per-class backoff strategy (exponential for timeouts, `Retry-After`-aware for 429/503)
- "Resume weeks later" integration test: simulate app restart after a long gap, confirm correct resume
- Automatic file renaming on output-name conflict (`file.zip` → `file (1).zip`)

DoD: integration test suite covers crash-mid-download, reboot-simulation, and stale-partial-file scenarios, all green in CI.

---

## Sprint 4 — Verification, Mirrors, Duplicate Detection
**Goal:** Downloaded files are provably correct; mirrors are used when available; duplicates are handled sanely.

Scope:
- Checksum verification: MD5/SHA1/SHA256/SHA512/CRC32 (post-download, optional pre-supplied hash)
- Per-chunk corruption detection + targeted re-download of only the bad chunk (chunk-level hashing during segmented download)
- Mirror support: accept multiple URLs for one Job, auto speed-compare, auto-switch on failure, continue partial download from a different mirror
- Duplicate detection: same URL / same hash / same filename in queue or history → prompt overwrite/rename/skip (CLI flag + API field for non-interactive policy)

DoD: a deliberately corrupted chunk is detected and only that chunk is re-fetched (verified via test that asserts byte-range of the re-fetch request); mirror failover test passes.

---

## Sprint 5 — Queue, Bandwidth Manager, Categories
**Goal:** Multiple downloads are managed sanely as a queue with bandwidth control.

Scope:
- Queue: add/pause/resume/cancel/reorder; priority levels (High/Normal/Low) feeding the scheduler's connection budget
- Nested queues + categories (Movies/Games/Images/Linux ISOs/Documents, user-definable)
- Bandwidth limiting: global cap, per-download cap, per-domain cap (token-bucket rate limiter shared across segments)
- Congestion auto-detection: rolling-window throughput drop triggers automatic backoff
- `crates/api`: REST endpoints for queue CRUD (`axum`), WebSocket channel streaming progress/queue events

DoD: 10 concurrent queued downloads respect a global bandwidth cap and per-item priority under load test; REST/WS API has OpenAPI-documented endpoints for queue management.

---

## Sprint 6 — Desktop App, Recovery, Release Packaging
**Goal:** Ship a real cross-platform desktop app and harden recovery, closing out Phase 1.

Scope:
- `apps/desktop`: Tauri 2 + React UI — add-download dialog, queue view, live speed graph, dark/light mode, notifications on completion/failure
- Tauri commands wire directly into `crates/engine` (no separate server process needed for the desktop app)
- Recovery: corrupted-database repair routine, orphaned temp-file cleanup, automatic settings/queue backup + session restore on launch
- Packaging: signed-where-possible installers for Windows (`.msi`), macOS (`.dmg`), Linux (`.AppImage` + `.deb`)
- CI: release workflow building all three platform artifacts on tag push

DoD: a person can install SmartDownloadManager from a release artifact on Windows, macOS, or Linux, add a download via the GUI, watch it progress with a live graph, quit mid-download, relaunch, and see it resume automatically. This is the Phase 1 MVP exit criteria from `PRD.md` §6.

---

## Cross-cutting (every sprint)
- Unit + integration tests required for new engine code (no PR merges without tests on `crates/engine`, `crates/protocols`)
- `docs/FEATURES.md` checkboxes updated in the same PR that completes a feature
- CI must be green before merge to `main`
