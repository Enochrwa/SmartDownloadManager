import { describe, expect, it } from "vitest";
import { DEFAULT_SETTINGS, InMemorySettingsStore, loadSettings, saveSettings } from "./settings";

describe("loadSettings", () => {
  it("returns defaults when nothing has been stored", async () => {
    const store = new InMemorySettingsStore();
    expect(await loadSettings(store)).toEqual(DEFAULT_SETTINGS);
  });

  it("layers stored values on top of the defaults", async () => {
    const store = new InMemorySettingsStore();
    await store.set({ clipboardMonitoringEnabled: true, pairingToken: "abc123" });
    const settings = await loadSettings(store);
    expect(settings.clipboardMonitoringEnabled).toBe(true);
    expect(settings.pairingToken).toBe("abc123");
    // Everything else still falls back to the default.
    expect(settings.minSizeBytes).toBe(DEFAULT_SETTINGS.minSizeBytes);
  });
});

describe("saveSettings", () => {
  it("persists a patch and returns the merged result", async () => {
    const store = new InMemorySettingsStore();
    const result = await saveSettings(store, { apiBaseUrl: "http://127.0.0.1:9999" });
    expect(result.apiBaseUrl).toBe("http://127.0.0.1:9999");

    // A second, independent load should see the same persisted value.
    const reloaded = await loadSettings(store);
    expect(reloaded.apiBaseUrl).toBe("http://127.0.0.1:9999");
  });

  it("merges successive patches rather than overwriting the whole object", async () => {
    const store = new InMemorySettingsStore();
    await saveSettings(store, { minSizeBytes: 1000 });
    const result = await saveSettings(store, { clipboardMonitoringEnabled: true });
    expect(result.minSizeBytes).toBe(1000);
    expect(result.clipboardMonitoringEnabled).toBe(true);
  });
});
