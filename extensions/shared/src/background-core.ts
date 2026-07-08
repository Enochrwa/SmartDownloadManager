/**
 * Shared background logic for both the Chrome (service worker) and
 * Firefox (event page) extension builds — Sprint 11's "WebExtensions
 * (Manifest V3) shared codebase". Each browser's `src/background.ts` is a
 * one-line entry point that calls `initBackground()`; everything that
 * actually does something lives here so the two builds can never drift
 * out of sync with each other.
 */

import { SdmApiClient } from "./api-client";
import { getBrowserApi } from "./browser-api";
import { ChromeStorageSettingsStore } from "./chrome-storage-adapter";
import { type ExtensionSettings, loadSettings } from "./settings";
import { extractUrls, shouldIntercept } from "./url-utils";

const CONTEXT_MENU_ID = "sdm-download-with";
const CLIPBOARD_ALARM_NAME = "sdm-clipboard-poll";
const OFFSCREEN_DOCUMENT_PATH = "offscreen.html";

async function currentClient(settingsStore = new ChromeStorageSettingsStore()): Promise<{
  client: SdmApiClient;
  settings: ExtensionSettings;
}> {
  const settings = await loadSettings(settingsStore);
  const client = new SdmApiClient(settings.apiBaseUrl, settings.pairingToken);
  return { client, settings };
}

/** Registers the right-click "Download with SmartDownloadManager" entry (Sprint 11 auto-detect scope). */
function setupContextMenu(): void {
  const api = getBrowserApi();
  api.contextMenus.removeAll(() => {
    api.contextMenus.create({
      id: CONTEXT_MENU_ID,
      title: "Download with SmartDownloadManager",
      contexts: ["link", "image", "video", "audio"],
    });
  });
}

async function handleContextMenuClick(info: chrome.contextMenus.OnClickData): Promise<void> {
  const url = info.linkUrl ?? info.srcUrl;
  if (!url) return;
  const { client } = await currentClient();
  try {
    await client.capture({
      url,
      pageUrl: info.pageUrl ?? null,
      suggestedFilename: null,
      sizeHintBytes: null,
      source: "context-menu",
    });
  } catch (err) {
    console.error("[sdm] context-menu capture failed", err);
  }
}

/**
 * `chrome.downloads.onCreated` + `chrome.downloads.cancel` is the standard
 * MV3-compatible technique for redirecting a browser download elsewhere:
 * `onDeterminingFilename` can *rename* a download but — in every
 * Chromium/Firefox version at time of writing — cannot reliably cancel
 * one, so interception has to happen a moment later, once the download
 * item actually exists.
 */
async function handleDownloadCreated(item: chrome.downloads.DownloadItem): Promise<void> {
  const api = getBrowserApi();
  const { client, settings } = await currentClient();
  const sizeHint = item.fileSize && item.fileSize > 0 ? item.fileSize : null;

  if (!shouldIntercept(item.url, sizeHint, settings)) {
    return;
  }

  try {
    await api.downloads.cancel(item.id);
    // Erase the (now-cancelled, effectively empty) entry from the
    // browser's own download shelf so it doesn't sit there as a
    // confusing "0 bytes, cancelled" row once sdmd has it instead.
    await api.downloads.erase({ id: item.id }).catch(() => undefined);
  } catch (err) {
    console.error("[sdm] failed to cancel intercepted download", err);
    return;
  }

  try {
    await client.capture({
      url: item.url,
      pageUrl: item.referrer ?? null,
      suggestedFilename: item.filename ? (item.filename.split(/[/\\]/).pop() ?? null) : null,
      sizeHintBytes: sizeHint,
      source: "download-intercept",
    });
  } catch (err) {
    console.error("[sdm] failed to hand intercepted download to sdmd", err);
  }
}

/**
 * Chrome's MV3 service worker has no `navigator.clipboard` (no DOM at
 * all); Firefox's MV3 background page does. `offscreen.html` is Chrome's
 * documented workaround — a hidden, DOM-having document a service worker
 * can talk to — for exactly this gap. `readClipboardText` tries the
 * direct route first so Firefox never needs the offscreen dance at all.
 */
async function readClipboardText(): Promise<string | null> {
  const nav = (globalThis as typeof globalThis & { navigator?: Navigator }).navigator;
  if (nav?.clipboard?.readText) {
    try {
      return await nav.clipboard.readText();
    } catch {
      // Fall through to the offscreen-document approach below — most
      // often this means we're in a Chrome service worker, where
      // `navigator.clipboard` doesn't exist and this branch was never
      // reachable in the first place.
    }
  }

  const api = getBrowserApi();
  if (!api.offscreen) return null;
  try {
    const existing = await api.offscreen.hasDocument?.();
    if (!existing) {
      await api.offscreen.createDocument({
        url: OFFSCREEN_DOCUMENT_PATH,
        reasons: ["CLIPBOARD" as chrome.offscreen.Reason],
        justification: "Read clipboard text to detect copied download links.",
      });
    }
    const response = (await api.runtime.sendMessage({ type: "sdm-read-clipboard" })) as
      | { text: string | null }
      | undefined;
    return response?.text ?? null;
  } catch (err) {
    console.error("[sdm] offscreen clipboard read failed", err);
    return null;
  }
}

let lastClipboardText: string | null = null;

async function pollClipboard(): Promise<void> {
  const { settings } = await currentClient();
  if (!settings.clipboardMonitoringEnabled) return;

  const text = await readClipboardText();
  if (!text || text === lastClipboardText) return;
  lastClipboardText = text;

  const urls = extractUrls(text);
  if (urls.length === 0) return;

  // Only the first URL gets a notification — a clipboard full of many
  // URLs is what the popup's batch-paste flow is for (Sprint 11 scope:
  // "detects a copied downloadable URL and shows a native browser
  // notification with a one-click 'Download?' action").
  const url = urls[0];
  const api = getBrowserApi();
  const notificationId = `sdm-clipboard-${Date.now()}`;
  pendingClipboardCaptures.set(notificationId, url);
  await api.notifications.create(notificationId, {
    type: "basic",
    iconUrl: "icons/icon128.png",
    title: "Download this link?",
    message: url,
    buttons: [{ title: "Download" }],
    priority: 1,
  });
}

const pendingClipboardCaptures = new Map<string, string>();

async function handleNotificationButtonClick(
  notificationId: string,
  buttonIndex: number,
): Promise<void> {
  if (buttonIndex !== 0) return;
  const url = pendingClipboardCaptures.get(notificationId);
  if (!url) return;
  pendingClipboardCaptures.delete(notificationId);

  const { client } = await currentClient();
  try {
    await client.capture({
      url,
      pageUrl: null,
      suggestedFilename: null,
      sizeHintBytes: null,
      source: "clipboard",
    });
  } catch (err) {
    console.error("[sdm] clipboard-notification capture failed", err);
  }
  getBrowserApi().notifications.clear(notificationId);
}

async function applyClipboardAlarmSchedule(): Promise<void> {
  const api = getBrowserApi();
  const { settings } = await currentClient();
  await api.alarms.clear(CLIPBOARD_ALARM_NAME);
  if (settings.clipboardMonitoringEnabled) {
    // Chrome enforces a minimum alarm period of 1 minute in production,
    // but honors sub-minute periods when the extension is unpacked/in
    // dev mode — falling back to whole minutes here keeps the schedule
    // valid either way rather than silently failing to register.
    const periodInMinutes = Math.max(settings.clipboardPollIntervalSeconds / 60, 1 / 60);
    api.alarms.create(CLIPBOARD_ALARM_NAME, { periodInMinutes });
  }
}

/** Entry point called by both `chrome/src/background.ts` and `firefox/src/background.ts`. */
export function initBackground(): void {
  const api = getBrowserApi();

  api.runtime.onInstalled.addListener(() => {
    setupContextMenu();
    void applyClipboardAlarmSchedule();
  });
  // Service workers restart frequently; re-apply the alarm schedule on
  // every startup too, not just on install.
  api.runtime.onStartup?.addListener(() => {
    void applyClipboardAlarmSchedule();
  });

  api.contextMenus.onClicked.addListener((info) => {
    void handleContextMenuClick(info);
  });

  api.downloads.onCreated.addListener((item) => {
    void handleDownloadCreated(item);
  });

  api.alarms.onAlarm.addListener((alarm) => {
    if (alarm.name === CLIPBOARD_ALARM_NAME) {
      void pollClipboard();
    }
  });

  api.notifications.onButtonClicked.addListener((notificationId, buttonIndex) => {
    void handleNotificationButtonClick(notificationId, buttonIndex);
  });

  // The options page saves settings via `chrome.storage.local` directly;
  // when the clipboard-monitoring toggle or interval changes we need to
  // re-schedule the alarm to match.
  api.storage.onChanged.addListener((changes, areaName) => {
    if (areaName === "local" && "sdmSettings" in changes) {
      void applyClipboardAlarmSchedule();
    }
  });

  // Messages from the popup: batch capture, single capture, pairing
  // verification. Each returns a Promise, which both `chrome.runtime`
  // and `browser.runtime` support as a `sendMessage` response.
  api.runtime.onMessage.addListener((message, _sender, sendResponse) => {
    if (!message || typeof message !== "object" || !("type" in message)) return undefined;
    handleRuntimeMessage(message as { type: string; [key: string]: unknown })
      .then(sendResponse)
      .catch((err) => sendResponse({ error: String(err) }));
    return true; // keep the message channel open for the async response
  });
}

async function handleRuntimeMessage(message: {
  type: string;
  [key: string]: unknown;
}): Promise<unknown> {
  const { client } = await currentClient();
  switch (message.type) {
    case "sdm-capture-batch": {
      const urls = message.urls as string[];
      const pageUrl = (message.pageUrl as string | null) ?? null;
      return client.captureBatch({ urls, pageUrl });
    }
    case "sdm-list-jobs":
      return client.listJobs();
    case "sdm-pairing-status":
      return client.pairingStatus();
    default:
      return undefined;
  }
}
