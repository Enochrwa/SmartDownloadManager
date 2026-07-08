/**
 * Shared options-page logic. Covers Sprint 11's settings surface:
 * interception (size threshold / file-type allowlist / per-site opt-in),
 * the clipboard-monitoring on/off toggle, and the pairing flow — pasting
 * in the token the desktop app displayed and confirming it against
 * `sdmd`.
 */

import { SdmApiClient } from "./api-client";
import { ChromeStorageSettingsStore } from "./chrome-storage-adapter";
import { type ExtensionSettings, loadSettings, saveSettings } from "./settings";

interface OptionsElements {
  apiBaseUrl: HTMLInputElement;
  pairingTokenInput: HTMLInputElement;
  pairButton: HTMLButtonElement;
  pairingStatus: HTMLElement;
  interceptEnabled: HTMLInputElement;
  minSizeMb: HTMLInputElement;
  fileTypeAllowlist: HTMLInputElement;
  perSiteOptIn: HTMLInputElement;
  clipboardMonitoringEnabled: HTMLInputElement;
  clipboardPollIntervalSeconds: HTMLInputElement;
  saveButton: HTMLButtonElement;
  saveConfirmation: HTMLElement;
}

function queryElements(root: ParentNode): OptionsElements {
  const req = <T extends Element>(selector: string): T => {
    const el = root.querySelector<T>(selector);
    if (!el) throw new Error(`options.html is missing required element: ${selector}`);
    return el;
  };
  return {
    apiBaseUrl: req<HTMLInputElement>("#sdm-api-base-url"),
    pairingTokenInput: req<HTMLInputElement>("#sdm-pairing-token"),
    pairButton: req<HTMLButtonElement>("#sdm-pair-button"),
    pairingStatus: req("#sdm-pairing-status"),
    interceptEnabled: req<HTMLInputElement>("#sdm-intercept-enabled"),
    minSizeMb: req<HTMLInputElement>("#sdm-min-size-mb"),
    fileTypeAllowlist: req<HTMLInputElement>("#sdm-file-type-allowlist"),
    perSiteOptIn: req<HTMLInputElement>("#sdm-per-site-opt-in"),
    clipboardMonitoringEnabled: req<HTMLInputElement>("#sdm-clipboard-enabled"),
    clipboardPollIntervalSeconds: req<HTMLInputElement>("#sdm-clipboard-interval"),
    saveButton: req<HTMLButtonElement>("#sdm-save-button"),
    saveConfirmation: req("#sdm-save-confirmation"),
  };
}

function csvToList(value: string): string[] {
  return value
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

function populateForm(els: OptionsElements, settings: ExtensionSettings): void {
  els.apiBaseUrl.value = settings.apiBaseUrl;
  els.pairingTokenInput.value = settings.pairingToken ?? "";
  els.interceptEnabled.checked = settings.interceptEnabled;
  els.minSizeMb.value = String(Math.round(settings.minSizeBytes / (1024 * 1024)));
  els.fileTypeAllowlist.value = settings.fileTypeAllowlist.join(", ");
  els.perSiteOptIn.value = settings.perSiteOptIn.join(", ");
  els.clipboardMonitoringEnabled.checked = settings.clipboardMonitoringEnabled;
  els.clipboardPollIntervalSeconds.value = String(settings.clipboardPollIntervalSeconds);
}

function formToPatch(els: OptionsElements): Partial<ExtensionSettings> {
  return {
    apiBaseUrl: els.apiBaseUrl.value.trim().replace(/\/+$/, ""),
    interceptEnabled: els.interceptEnabled.checked,
    minSizeBytes: Math.max(0, Number(els.minSizeMb.value) || 0) * 1024 * 1024,
    fileTypeAllowlist: csvToList(els.fileTypeAllowlist.value).map((s) => s.toLowerCase()),
    perSiteOptIn: csvToList(els.perSiteOptIn.value),
    clipboardMonitoringEnabled: els.clipboardMonitoringEnabled.checked,
    clipboardPollIntervalSeconds: Math.max(1, Number(els.clipboardPollIntervalSeconds.value) || 2),
  };
}

export async function initOptions(root: ParentNode = document): Promise<void> {
  const els = queryElements(root);
  const store = new ChromeStorageSettingsStore();
  let settings = await loadSettings(store);
  populateForm(els, settings);

  async function refreshPairingStatus(): Promise<void> {
    const client = new SdmApiClient(els.apiBaseUrl.value.trim(), settings.pairingToken);
    try {
      const status = await client.pairingStatus();
      els.pairingStatus.textContent = status.connected
        ? `Connected (${status.pairedExtensions.length} paired extension${status.pairedExtensions.length === 1 ? "" : "s"})`
        : "Not currently connected";
    } catch {
      els.pairingStatus.textContent = "Can't reach sdmd at this address";
    }
  }

  els.pairButton.addEventListener("click", () => {
    const token = els.pairingTokenInput.value.trim();
    if (!token) {
      els.pairingStatus.textContent = "Paste the token shown in the desktop app first";
      return;
    }
    const client = new SdmApiClient(els.apiBaseUrl.value.trim(), null);
    els.pairingStatus.textContent = "Verifying…";
    void client
      .verifyPairingToken(token)
      .then(async (result) => {
        if (!result.ok) {
          els.pairingStatus.textContent = "That token was rejected — check it and try again";
          return;
        }
        settings = await saveSettings(store, { pairingToken: token });
        els.pairingStatus.textContent = "Paired successfully";
        await refreshPairingStatus();
      })
      .catch(() => {
        els.pairingStatus.textContent = "Couldn't reach sdmd at this address";
      });
  });

  els.saveButton.addEventListener("click", () => {
    void (async () => {
      settings = await saveSettings(store, formToPatch(els));
      els.saveConfirmation.textContent = "Saved";
      setTimeout(() => {
        els.saveConfirmation.textContent = "";
      }, 1500);
      await refreshPairingStatus();
    })();
  });

  await refreshPairingStatus();
}
