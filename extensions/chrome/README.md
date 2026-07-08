# Chrome Extension (Manifest V3)

Implemented in Phase 2, Sprint 11 (`docs/SPRINT_PLAN_PHASE2.md`). Shares a
TypeScript core with `extensions/firefox` (see `extensions/shared`) — download
interception, a right-click "Download with SmartDownloadManager" context
menu, clipboard monitoring with a one-click "Download?" notification, drag &
drop onto the popup, and batch URL detection from pasted text — talking
exclusively to `sdmd`'s REST/WebSocket API (`crates/server`), authenticated
via the pairing-token flow the desktop app's Settings panel exposes.

## Building

```sh
pnpm install
pnpm --filter @sdm/extension-chrome build
```

This produces a loadable unpacked extension in `dist/`.

## Loading it in Chrome

1. Run the desktop app (or the standalone `sdmd` binary) so there's something
   for the extension to pair with.
2. Visit `chrome://extensions`, enable "Developer mode", click "Load
   unpacked", and select `extensions/chrome/dist`.
3. Open the extension's options page, and in the desktop app's Settings →
   Browser Extension, click "Generate pairing token". Paste the token into
   the options page and click "Pair".
4. Click a direct-download link on any page — it should show up in the
   desktop app's queue instead of Chrome's own download shelf.

## Layout

- `src/background.ts`, `src/popup.ts`, `src/options.ts`, `src/offscreen.ts` —
  thin entry points; all real logic lives in `extensions/shared/src`
  (`background-core.ts`, `popup-core.ts`, `options-core.ts`,
  `offscreen-core.ts`).
- `public/` — `manifest.json`, the three HTML pages, and icons, copied
  verbatim into `dist/` by the Vite build.
- `offscreen.html`/`offscreen.ts` are Chrome-specific: MV3 service workers
  have no DOM (and therefore no `navigator.clipboard`), so clipboard reads
  are delegated to this hidden document. Firefox's background page has a DOM
  directly and never loads it — see `extensions/firefox/README.md`.
