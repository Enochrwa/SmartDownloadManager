import { getBrowserApi } from "./browser-api";
import type { ExtensionSettings, SettingsStore } from "./settings";

const STORAGE_KEY = "sdmSettings";

/**
 * Real `SettingsStore` backed by `chrome.storage.local` (works identically
 * against Firefox's `browser.storage.local` via `getBrowserApi`). Settings
 * — including the pairing token — are deliberately kept in `local` rather
 * than `sync` storage: a pairing token is only valid for one specific
 * `sdmd` instance, and syncing it to the same browser profile on another
 * machine would just produce a token that doesn't authenticate anywhere.
 */
export class ChromeStorageSettingsStore implements SettingsStore {
  async get(): Promise<Partial<ExtensionSettings>> {
    const api = getBrowserApi();
    const result = await api.storage.local.get(STORAGE_KEY);
    return (result[STORAGE_KEY] as Partial<ExtensionSettings> | undefined) ?? {};
  }

  async set(patch: Partial<ExtensionSettings>): Promise<void> {
    const api = getBrowserApi();
    const existing = await this.get();
    await api.storage.local.set({ [STORAGE_KEY]: { ...existing, ...patch } });
  }
}
