# Technology Decisions

An audit of every major technology choice: what we kept, what we changed, and
why — plus the alternatives we rejected and the conditions under which we'd
revisit them. Written so a future contributor (or a future us) doesn't have to
re-litigate these.

Status: **Revision 2**, post-bootstrap audit. Revision 1 was the initial
bootstrap; this revision tightens the daemon/CLI split, locks the BitTorrent
crate decision, and adds toolchain/repo-hygiene infrastructure that a project
aiming to be "the best downloader in the world" can't skip.

---

## 1. Core engine language: Rust — **confirmed**

Alternatives considered: Go, C++, Zig.

- **Go** has the easiest concurrency model but its GC introduces latency
  jitter that's a real problem at 128-connection segment-stealing scale, and
  its FFI story for FFmpeg/libtorrent-style integration is weaker than Rust's.
- **C++** is what aria2/qBittorrent are written in — proven, but memory-safety
  bugs in a tool that handles untrusted server responses across a dozen
  protocols is exactly the risk class Rust exists to eliminate. Not worth it
  in a greenfield project.
- **Zig** is interesting for systems work but the ecosystem (async runtime,
  HTTP/TLS/SSH/BitTorrent crates) doesn't exist yet at the depth Rust's does.

Rust wins on memory safety for untrusted network input, a mature async
ecosystem (tokio), and one toolchain that cross-compiles cleanly to Windows/
macOS/Linux.

## 2. Desktop shell: Tauri 2 — **confirmed**

Alternatives considered: Electron, native (per-platform), Flutter.

- **Electron** ships a full Chromium + Node per app (100MB+ baseline,
  meaningful memory overhead) and requires a second IPC bridge to reach a
  Rust core. Tauri's Rust backend *is* the engine's host process — no bridge.
- **Native per-platform** (WinUI/AppKit/GTK) gives the best polish but
  triples UI development effort for a small team. Not viable for v1.
- **Flutter** has good cross-platform UI but its Rust interop (via FFI) adds
  friction the Tauri approach doesn't have, and the team's frontend expertise
  is React/TypeScript, not Dart.

Validating data point: `librqbit` (the BitTorrent library we're adopting,
see §4) ships its own reference desktop app built on Tauri — independent
confirmation this pairing is proven for exactly this problem domain.

## 3. Frontend: React + TypeScript — **confirmed**

Alternatives considered: Svelte, SolidJS, Vue.

- SolidJS has better raw rendering performance for high-frequency updates
  (live progress bars at 10+ Hz across dozens of queue rows), which is a
  genuinely good argument for a downloader UI specifically.
- We're keeping React anyway: it matches existing team expertise (used
  across the team's other production apps with Redux, TanStack Query, and
  MUI), has the deepest component ecosystem, and the live-progress
  performance concern is solvable within React (windowed lists for the queue,
  `requestAnimationFrame`-batched progress updates) without paying a
  framework-switch cost. Revisit only if profiling in Sprint 6 shows React
  genuinely can't hit smooth 60fps with a large queue.

**Change in this revision:** adding **Zustand** for client-side queue/job
state (replacing ad hoc `useState`) — a lighter fit than Redux for a
WebSocket-driven, frequently-updating state shape, with far less boilerplate.
TanStack Query stays in the toolbox for anything that's request/response
rather than streaming (settings, history search).

## 4. BitTorrent: `librqbit` — **locked in (was tentative)**

Alternatives considered: `cratetorrent`, bindings to C++ `libtorrent`.

Verified before locking this in: `librqbit` is Apache-2.0, actively
maintained with releases continuing into 2026, used as a dependency by over a
dozen other crates, and — notably — is the library backing `rqbit`, which
itself ships CLI, HTTP API, Web UI, *and* a Tauri desktop app. That's the
exact same shape of project we're building, already proven on this stack.
`cratetorrent` is comparatively inactive. C++ `libtorrent` bindings would
reintroduce the memory-safety and build-complexity problems Rust was chosen
to avoid. Decision: **`librqbit`, no longer "to be confirmed in Sprint 7."**

## 5. HLS/DASH parsing: `m3u8-rs` + `dash-mpd-rs` — **added (was unspecified)**

Revision 1 said "Phase 2, TBD." That's not good enough for a project whose
entire pitch is matching/beating yt-dlp's stream-capture breadth. Locking in
`m3u8-rs` (MIT) for HLS playlist parsing and `dash-mpd-rs` (MIT) for MPEG-DASH
manifest parsing — both pure-Rust, both actively used in the Rust media
tooling space — feeding segments into `crates/protocols`' existing HTTP
downloader and `crates/media`'s FFmpeg remux step.

## 6. Daemon/CLI split — **new in this revision**

Revision 1 had a single `crates/api` crate that was simultaneously "the CLI"
and "the REST/WS server," which is a real design flaw: a one-shot CLI
download and a long-running API daemon have different lifecycle, process
model, and packaging needs, and Sprint 5 was implicitly going to have to
un-tangle them mid-sprint anyway.

Fixed by splitting into three crates (see `docs/ARCHITECTURE.md` for the
diagram):

- **`crates/api-types`** — shared request/response DTOs (serde structs), no
  logic. Used by both the daemon and the CLI, and later by SDKs.
- **`crates/server`** (binary: `sdmd`) — the long-running daemon: owns the
  engine, exposes it over REST + WebSocket (axum). This is what the browser
  extension and any future mobile client talk to.
- **`crates/cli`** (binary: `sdm`) — for one-shot scripting, drives the
  engine **in-process** directly (no daemon required for `sdm download
  <url>` to just work). If a `sdmd` daemon is already running, `sdm` can
  instead talk to it over the REST API for queue inspection — this mirrors
  how `aria2c`/`aria2rpc` and `rqbit`'s own CLI-vs-server split work, which is
  a validated pattern for exactly this kind of tool.

This also cleanly resolves how the browser extension (Phase 2) and a future
mobile client (Phase 5) reach the engine: they always talk to `sdmd`, never
to the desktop app's in-process engine, so a headless server install (e.g. on
a home NAS) is a natural byproduct rather than a retrofit.

## 7. Repo task runner: Turborepo — **added (was raw `pnpm -r`)**

Plain `pnpm -r build/lint/test` re-runs everything, every time, with no
caching or dependency-aware ordering. For a monorepo with 4 apps-worth of
crates and multiple JS packages, that gets slow fast and wastes CI minutes.
Turborepo (MIT, from Vercel) adds local + remote build caching and a proper
task graph for free. Rejected alternative: **Nx** — more powerful but heavier
configuration surface than this repo needs; Turborepo's simplicity wins for
our package count.

## 8. JS/TS lint+format: Biome — **added (replacing implied ESLint+Prettier)**

Revision 1's `package.json` scripts referenced `eslint` without ever
configuring it, and didn't budget for Prettier at all. Rather than configure
two separate tools with overlapping concerns, we're standardizing on
**Biome** (MIT): one fast (Rust-implemented) tool for both linting and
formatting, single config file, no plugin sprawl. It also thematically fits a
Rust-core project. Revisit only if a specific ESLint plugin we need has no
Biome equivalent (none identified yet for this project's needs).

## 9. Frontend testing: Vitest + React Testing Library — **added (was unconfigured)**

Revision 1's root `pnpm test` script had nothing to run. Vitest (Vite-native,
fast, Jest-compatible API) + React Testing Library is the standard pairing
for a Vite/React app and was simply missing. Added with a real sample test in
this revision.

## 10. Rust test runner: `cargo-nextest` — **added in CI**

Faster, better-isolated test execution than `cargo test`, with cleaner CI
output. Drop-in for CI; `cargo test` still works locally without installing
anything extra.

## 11. License compliance: `cargo-deny` — **added**

Given this project deliberately straddles MIT/Apache-2.0 (our code), Unlicense
(yt-dlp, subprocess), and LGPL/GPL (FFmpeg, subprocess) — see
`docs/LICENSING.md` — we now run `cargo-deny check licenses` in CI on every
PR, with an explicit allow-list, so a contributor can't accidentally add a
GPL-licensed *linked* dependency without it being caught immediately rather
than discovered at release-audit time.

## 12. Build automation: `cargo xtask` pattern — **added**

Cross-platform dev/release scripting (codesigning prep, bundling, asset
generation) needs to run identically on Windows/macOS/Linux. Shell scripts
don't. The Rust community's `cargo xtask` convention (a plain binary crate
invoked via `cargo xtask <command>`) avoids adding Make/Just/another tool
dependency — it's just more Rust, runs everywhere `cargo` does.

## 13. Storage: SQLite via `sqlx` — **confirmed**

Alternative considered: `redb` (pure-Rust embedded KV store).

`redb` is appealing for being pure-Rust with no C dependency, but the
Search feature (§13 of the feature list — regex search across history,
filtering, sorting) and Analytics (§15 — aggregation queries) both want
real relational queries. SQLite's query surface is the right tool; the
"pure Rust" argument isn't worth giving up SQL for.

---

## Summary of changes in this revision

| Area | Before | After |
|---|---|---|
| API/CLI | One `crates/api` crate, conflated | `crates/api-types` + `crates/server` (`sdmd`) + `crates/cli` (`sdm`) |
| BitTorrent crate | "TBD, confirm before Sprint 7" | Locked: `librqbit` |
| HLS/DASH crates | Unspecified | Locked: `m3u8-rs`, `dash-mpd-rs` |
| Task runner | `pnpm -r` | Turborepo |
| JS lint/format | Implied ESLint (unconfigured) | Biome |
| Frontend tests | Unconfigured | Vitest + React Testing Library |
| Rust test runner (CI) | `cargo test` | `cargo-nextest` |
| License compliance | Manual (`docs/LICENSING.md` only) | `cargo-deny` enforced in CI |
| Cross-platform scripting | Implied shell scripts | `cargo xtask` |
| Toolchain pinning | None | `rust-toolchain.toml` |
| Client state (frontend) | `useState` | Zustand |
