/**
 * The WebExtensions API is `chrome.*` in every Chromium-based browser
 * (Chrome, Edge, Brave, Opera, Vivaldi) and `browser.*` in Firefox.
 * Rather than pull in the `webextension-polyfill` package, this repo's
 * shared codebase (per Sprint 11's "shared codebase... Firefox's MV3
 * support gets a compatibility shim" scope note) just resolves whichever
 * global is present once, here, and every other module imports
 * `browserApi` instead of referencing `chrome`/`browser` directly.
 */

// Both browsers expose a `chrome`-shaped API (Firefox's `browser` object
// is intentionally near-identical, with the difference that most of its
// methods return Promises natively instead of taking callbacks — this
// codebase only ever uses the Promise-returning form of each API, which
// both browsers support, so the same calling code works unmodified
// against either global).
export function getBrowserApi(): typeof chrome {
  const g = globalThis as typeof globalThis & {
    chrome?: typeof chrome;
    browser?: typeof chrome;
  };
  const api = g.chrome ?? g.browser;
  if (!api) {
    throw new Error(
      "no WebExtensions API found on globalThis (neither `chrome` nor `browser`) — " +
        "this module must run inside an extension context",
    );
  }
  return api;
}
