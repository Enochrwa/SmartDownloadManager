/**
 * Runs inside `offscreen.html`, a hidden document Chrome's MV3 service
 * worker spins up specifically so extension code can reach DOM-only APIs
 * like `navigator.clipboard` — see `background-core.ts::readClipboardText`
 * for why this indirection is necessary at all. Firefox's background page
 * has a DOM directly and never loads this document.
 */
import { getBrowserApi } from "./browser-api";

export function initOffscreen(): void {
  const api = getBrowserApi();
  api.runtime.onMessage.addListener((message, _sender, sendResponse) => {
    if (!message || typeof message !== "object" || message.type !== "sdm-read-clipboard") {
      return undefined;
    }
    navigator.clipboard
      .readText()
      .then((text) => sendResponse({ text }))
      .catch(() => sendResponse({ text: null }));
    return true;
  });
}
