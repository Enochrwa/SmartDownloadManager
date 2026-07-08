# Running & Testing SmartDownloadManager (Sprint 11: Desktop + Browser Extensions)

This guide takes you from a fresh clone to a working desktop app paired with
a live browser extension, on **Windows**, **macOS**, and **Ubuntu/Linux**,
plus how to exercise every capture path (interception, context menu,
clipboard, drag & drop, batch paste) and run the full test suite.

- [1. Prerequisites](#1-prerequisites)
  - [Ubuntu / Linux](#ubuntu--linux)
  - [macOS](#macos)
  - [Windows](#windows)
- [2. Clone and install](#2-clone-and-install)
- [3. Run the desktop app](#3-run-the-desktop-app)
- [4. Build the browser extensions](#4-build-the-browser-extensions)
- [5. Load the extension in your browser](#5-load-the-extension-in-your-browser)
- [6. Pair the extension with the desktop app](#6-pair-the-extension-with-the-desktop-app)
- [7. Test every capture path](#7-test-every-capture-path)
- [8. Run the automated test suites](#8-run-the-automated-test-suites)
- [9. Optional: standalone daemon + CLI, no desktop UI](#9-optional-standalone-daemon--cli-no-desktop-ui)
- [10. Troubleshooting](#10-troubleshooting)

---

## 1. Prerequisites

You need **Rust** (stable, 1.80+), **Node.js 20+**, and **pnpm 9**, plus a
few Tauri system dependencies that differ per OS.

### Ubuntu / Linux

```bash
# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustup component add rustfmt clippy

# Node + pnpm
curl -fsSL https://deb.nodesource.com/setup_20.x | sudo -E bash -
sudo apt-get install -y nodejs
npm install -g pnpm@9.0.0

# Tauri's Linux build dependencies (WebKitGTK, WebKit's JS engine, etc.)
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libjavascriptcoregtk-4.1-dev \
  libsoup-3.0-dev \
  libssl-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libxdo-dev \
  build-essential curl wget file
```

### macOS

```bash
# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustup component add rustfmt clippy

# Xcode command line tools (provides the linker/frameworks Tauri needs)
xcode-select --install

# Node + pnpm (via Homebrew)
brew install node@20
npm install -g pnpm@9.0.0
```

No extra system libraries are needed beyond Xcode CLT — Tauri uses the
system WebView (WKWebView) on macOS.

### Windows

1. Install **Rust**: download and run [rustup-init.exe](https://rustup.rs),
   accept the default (MSVC) toolchain.
2. Install **Node.js 20 LTS** from [nodejs.org](https://nodejs.org), then:
   ```powershell
   npm install -g pnpm@9.0.0
   ```
3. Install **"Desktop development with C++"** via the
   [Visual Studio Build Tools](https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio)
   installer (Tauri needs the MSVC linker).
4. Install **WebView2** — already preinstalled on Windows 11 and most
   updated Windows 10 systems; if missing, get it from
   [Microsoft's WebView2 page](https://developer.microsoft.com/microsoft-edge/webview2/).

Run all commands below from **PowerShell** (or the Tauri-recommended
"x64 Native Tools Command Prompt" if you hit linker errors).

---

## 2. Clone and install

Same on every platform:

```bash
git clone https://github.com/Enochrwa/SmartDownloadManager.git
cd SmartDownloadManager
pnpm install
```

---

## 3. Run the desktop app

```bash
pnpm tauri dev
```

- First run compiles the Rust side — expect several minutes.
- This starts the desktop UI **and** an embedded extension API on
  `http://127.0.0.1:7890` in the same process — no separate daemon needed.
- Leave this running for every step below.

Windows note: if `pnpm tauri dev` fails to find a linker, re-run it from the
**"x64 Native Tools Command Prompt for VS"** instead of a plain terminal.

---

## 4. Build the browser extensions

```bash
pnpm --filter @sdm/extension-chrome build
pnpm --filter @sdm/extension-firefox build
```

Produces loadable, unpacked extensions at:

- `extensions/chrome/dist/` — for Chrome, Edge, Brave, Opera, Vivaldi
- `extensions/firefox/dist/` — for Firefox

(Re-run these commands after any code change — nothing auto-rebuilds.)

---

## 5. Load the extension in your browser

Identical steps on Windows/macOS/Ubuntu — only the browser's own UI differs.

### Chrome / Edge / Brave / Vivaldi (Chromium-based)

1. Navigate to `chrome://extensions` (Edge: `edge://extensions`, Brave:
   `brave://extensions`).
2. Enable **"Developer mode"** (toggle, usually top-right).
3. Click **"Load unpacked"** and select the `extensions/chrome/dist` folder.
4. The SmartDownloadManager icon appears in your toolbar.

### Firefox

1. Navigate to `about:debugging#/runtime/this-firefox`.
2. Click **"Load Temporary Add-on…"**.
3. Select the file `extensions/firefox/dist/manifest.json` (not the folder).
4. This is temporary — Firefox unloads it on restart; reload it each session,
   or use `web-ext run` for a persistent dev session if you have
   [`web-ext`](https://github.com/mozilla/web-ext) installed:
   ```bash
   npx web-ext run --source-dir extensions/firefox/dist
   ```

### Opera

Opera supports Chrome extensions natively — go to
`opera://extensions`, enable Developer Mode, "Load unpacked", same as
Chrome above.

---

## 6. Pair the extension with the desktop app

1. In the running desktop app, open **Settings → Browser Extension**.
2. Click **"Generate pairing token"**. You'll see a token and the address it
   expects (`http://127.0.0.1:7890` by default).
3. Click the extension's toolbar icon → the popup's **"Settings"** link (or
   right-click the icon → **Options**/**Manage Extension** → find its
   options page).
4. Paste the token into **"Pairing token"**, confirm the **"sdmd address"**
   field matches what the desktop app showed, click **"Pair"**.
5. You should see **"Paired successfully"**. Back in the desktop app, the
   dot next to "Browser Extension" turns green within ~3 seconds
   ("Extension connected").

If pairing fails with "Couldn't reach sdmd at this address", the desktop
app either isn't running or something else is using port 7890 (see
[Troubleshooting](#10-troubleshooting)).

---

## 7. Test every capture path

With the desktop app running and the extension paired, try each of these.
Every one should make a new entry appear in the desktop app's queue (and in
the popup's own recent-jobs list) — **not** in the browser's native
download manager.

### 7.1 Download interception

By default, interception fires for files **≥ 5 MB**, or any file with
extension `zip / iso / mp4 / mkv / dmg / exe / pdf` regardless of size.

- Click a direct-download link to a large file, or any `.zip`/`.pdf`/etc.
- **Expected:** it does *not* show up in the browser's own download
  bar/shelf; it appears in the desktop app's queue immediately.
- Tune the threshold, allowlist, or add specific sites to always intercept
  from in the extension's **Options** page.

### 7.2 Right-click context menu

- Right-click any link, image, video, or audio element on a page.
- Choose **"Download with SmartDownloadManager"**.
- **Expected:** new job appears in the queue.

### 7.3 Clipboard monitoring (off by default)

- In the extension's Options page, enable **"Notify me when a downloadable
  link is copied"**.
- Copy any `http(s)://` URL to your clipboard (select text + Ctrl/Cmd+C, or
  right-click a link → "Copy link").
- **Expected:** within ~2 seconds, a native OS/browser notification appears
  ("Download this link?") with a **"Download"** button. Click it.
- **Expected:** job appears in the queue.

  > **Platform note:** the first time this runs, your OS/browser may prompt
  > for notification permission — accept it, or you won't see the popup.
  > On Chrome, clipboard reads happen via a hidden "offscreen document";
  > on Firefox they happen directly in the background script — both are
  > automatic, no extra setup either way.

### 7.4 Drag & drop

- Click the extension's toolbar icon to open the popup.
- Drag a link (or a text selection containing a URL) from the page onto the
  **"Or drop a link here"** box in the popup.
- **Expected:** job appears in the queue.

### 7.5 Batch URL detection (paste)

- Open the popup, paste a block of text containing several URLs into the
  textarea (e.g. copy several lines from a document or webpage), click
  **"Queue links"**.
- **Expected:** every distinct URL becomes its own job; exact duplicates
  within the same paste collapse to a single job.

### 7.6 Live status in the popup

- With any download in progress, reopen the popup — it polls every 2
  seconds and shows connection status plus each job's live percentage.

---

## 8. Run the automated test suites

Same commands on all three platforms (Windows: use PowerShell or Git Bash).

```bash
# --- Rust ---
# Full workspace (engine, storage, protocols, media, torrent, server, CLI...)
cargo test --workspace

# Just the Sprint 11 server: real HTTP downloads through the full stack
# (auth, pairing flow, job lifecycle, capture dedup) via wiremock
cargo test -p sdm-server

# Just the new pairing-token storage tests
cargo test -p sdm-storage pairing_token

# Lint + format checks (must be clean, CI enforces both)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check

# --- Frontend (desktop app + extensions + shared packages) ---
pnpm test          # runs every package's test suite via Turborepo
pnpm build         # builds every package, including both extensions
pnpm lint          # Biome lint across the whole repo

# Just the extension's pure-logic unit tests (URL parsing, dedup,
# interception heuristics, settings, API client)
pnpm --filter @sdm/extension-shared test

# Just the desktop app's frontend tests (includes the pairing UI test)
pnpm --filter @sdm/desktop test
```

Expected result: **0 failures** across all of the above; clippy and fmt
report no issues.

---

## 9. Optional: standalone daemon + CLI, no desktop UI

If you'd rather not run the desktop app at all, you can run the daemon
directly and drive it with the CLI or the extension:

```bash
# Terminal 1: the daemon (same REST/WS API, standalone)
cargo run -p sdm-server
# listens on http://0.0.0.0:7890 by default; override with SDM_API_PORT

# Terminal 2: CLI usage (unrelated to pairing — CLI talks to the engine
# in-process via its own binary, not over HTTP)
cargo run -p sdm-cli -- add https://example.com/file.zip
```

Note: **the pairing flow's "Generate pairing token" button only exists in
the desktop app's Settings panel.** If you run the standalone `sdmd`
binary instead, mint a token directly via curl (loopback-only, matching
the same rule the desktop app follows):

```bash
curl -X POST http://127.0.0.1:7890/pairing/tokens \
  -H "content-type: application/json" \
  -d '{"label": "manual pairing"}'
# -> {"token": "...", "label": "manual pairing", "created_at": "..."}
```

Paste the returned `token` into the extension's Options page as usual.

---

## 10. Troubleshooting

| Symptom | Fix |
|---|---|
| Extension popup/options says "Can't reach sdmd" | The desktop app (or standalone `sdmd`) isn't running, or something else owns port 7890. Set `SDM_EXTENSION_API_PORT=<port>` (desktop app) or `SDM_API_PORT=<port>` (standalone `sdmd`) before launching, and update the extension's "sdmd address" field to match. |
| "That token was rejected" during pairing | Token was mistyped/truncated when copy-pasting, or was already revoked — generate a fresh one and retry. |
| "Extension connected" dot stays gray/red | Give it a few seconds (status polls every 3s in the desktop app, 2s in the popup). Reconfirm the extension's popup itself shows "Connected to SmartDownloadManager". |
| Interception never triggers | Check the file's size/extension against your threshold and allowlist in the extension's Options page — small, non-allowlisted files are intentionally left to the browser. |
| No clipboard notification appears | Confirm the toggle is enabled in Options; check your OS/browser hasn't blocked notification permission for the extension (Chrome: `chrome://settings/content/notifications`; Firefox: `about:preferences#privacy` → Permissions → Notifications). |
| `pnpm tauri dev` fails to link on Windows | Re-run from the "x64 Native Tools Command Prompt for VS" so the MSVC linker is on `PATH`. |
| Tauri build fails on Ubuntu with missing `webkit2gtk`/`javascriptcoregtk` | Re-check the `apt-get install` list in [Ubuntu / Linux](#ubuntu--linux) — package names vary slightly across Ubuntu versions; `apt search webkit2gtk` to find the exact version available. |
| `cargo test -p sdm-server` fails with an OpenSSL build error | Install `libssl-dev`/`pkg-config` (Linux) — some transitive dependencies still link OpenSSL even though the project's own HTTP client uses rustls. |
| Firefox extension disappears after restart | Expected — `about:debugging` temporary add-ons don't persist. Reload it, or package/sign it for permanent installation if you need that. |
