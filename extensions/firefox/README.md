# Firefox Extension (Manifest V3)

Implemented in Phase 2, Sprint 11 (`docs/SPRINT_PLAN_PHASE2.md`). See
`extensions/chrome` for the shared approach — the same `extensions/shared`
TypeScript core drives both builds, so Firefox, Edge, Brave, Opera, and
Vivaldi all stay in sync automatically. `extensions/shared/src/browser-api.ts`
resolves whichever of `chrome`/`browser` is present at runtime rather than
requiring a separate polyfill dependency.

## Building

```sh
pnpm install
pnpm --filter @sdm/extension-firefox build
```

## Loading it in Firefox

1. Run the desktop app (or `sdmd`) so there's something to pair with.
2. Visit `about:debugging#/runtime/this-firefox`, click "Load Temporary
   Add-on…", and select `extensions/firefox/dist/manifest.json`.
3. Complete the pairing flow from the extension's options page exactly as
   described in `extensions/chrome/README.md`.

## Why there's no `offscreen.ts` here

Chrome's MV3 service worker has no DOM at all, which is why the Chrome build
needs an offscreen document just to call `navigator.clipboard.readText()`.
Firefox's MV3 background page is a real (if not always visible) page with a
DOM, so `background-core.ts`'s clipboard read works directly there — the
offscreen-document code path in `background-core.ts` is simply never
exercised on this build.
