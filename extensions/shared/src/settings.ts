import type { InterceptionSettings } from "./url-utils";

/**
 * Everything the options page exposes, per Sprint 11's scope: size
 * threshold / file-type allowlist / per-site opt-in for interception, a
 * clipboard-monitoring on/off toggle (off by default — "clipboard access
 * is sensitive"), and the pairing token + `sdmd` address the extension
 * authenticates with.
 */
export interface ExtensionSettings extends InterceptionSettings {
  clipboardMonitoringEnabled: boolean;
  /** Seconds between clipboard polls when monitoring is enabled. */
  clipboardPollIntervalSeconds: number;
  /** e.g. "http://127.0.0.1:7890" — no trailing slash. */
  apiBaseUrl: string;
  /** Bearer token from the pairing flow; null until paired. */
  pairingToken: string | null;
}

export const DEFAULT_SETTINGS: ExtensionSettings = {
  interceptEnabled: true,
  // 5 MiB: small enough to catch most media/archive downloads, large
  // enough not to intercept every tiny favicon/thumbnail-sized GET.
  minSizeBytes: 5 * 1024 * 1024,
  fileTypeAllowlist: ["zip", "iso", "mp4", "mkv", "dmg", "exe", "pdf"],
  perSiteOptIn: [],
  // Off by default: clipboard access is sensitive (Sprint 11 scope note),
  // so the user has to explicitly opt in from the options page.
  clipboardMonitoringEnabled: false,
  clipboardPollIntervalSeconds: 2,
  apiBaseUrl: "http://127.0.0.1:7890",
  pairingToken: null,
};

/**
 * Minimal storage abstraction so settings logic is unit-testable without
 * `chrome.storage` — `chrome-storage-adapter.ts` (extension-side, not
 * unit-tested directly) implements this against the real API; tests use
 * a trivial in-memory `Map`-backed implementation instead.
 */
export interface SettingsStore {
  get(): Promise<Partial<ExtensionSettings>>;
  set(patch: Partial<ExtensionSettings>): Promise<void>;
}

export async function loadSettings(store: SettingsStore): Promise<ExtensionSettings> {
  const stored = await store.get();
  return { ...DEFAULT_SETTINGS, ...stored };
}

export async function saveSettings(
  store: SettingsStore,
  patch: Partial<ExtensionSettings>,
): Promise<ExtensionSettings> {
  await store.set(patch);
  return loadSettings(store);
}

/** In-memory `SettingsStore` used by unit tests and as a popup-preview fallback. */
export class InMemorySettingsStore implements SettingsStore {
  private data: Partial<ExtensionSettings> = {};

  async get(): Promise<Partial<ExtensionSettings>> {
    return { ...this.data };
  }

  async set(patch: Partial<ExtensionSettings>): Promise<void> {
    this.data = { ...this.data, ...patch };
  }
}
