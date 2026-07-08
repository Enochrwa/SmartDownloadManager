/**
 * Shared popup logic for both browser builds. `chrome/src/popup.ts` and
 * `firefox/src/popup.ts` are one-liners that call `initPopup()` once
 * `popup.html` has loaded; everything else — pairing status, the
 * batch-paste box, the drag & drop zone, and the recent-jobs list — lives
 * here.
 */

import { SdmApiClient } from "./api-client";
import { ChromeStorageSettingsStore } from "./chrome-storage-adapter";
import { loadSettings } from "./settings";
import { dedupeUrls, extractUrls } from "./url-utils";

interface PopupElements {
  statusDot: HTMLElement;
  statusText: HTMLElement;
  pasteArea: HTMLTextAreaElement;
  queueButton: HTMLButtonElement;
  dropZone: HTMLElement;
  jobsList: HTMLElement;
  optionsLink: HTMLAnchorElement;
}

function queryElements(root: ParentNode): PopupElements {
  const req = <T extends Element>(selector: string): T => {
    const el = root.querySelector<T>(selector);
    if (!el) throw new Error(`popup.html is missing required element: ${selector}`);
    return el;
  };
  return {
    statusDot: req("#sdm-status-dot"),
    statusText: req("#sdm-status-text"),
    pasteArea: req<HTMLTextAreaElement>("#sdm-paste-area"),
    queueButton: req<HTMLButtonElement>("#sdm-queue-button"),
    dropZone: req("#sdm-drop-zone"),
    jobsList: req("#sdm-jobs-list"),
    optionsLink: req<HTMLAnchorElement>("#sdm-options-link"),
  };
}

function renderJobs(
  list: HTMLElement,
  jobs: {
    id: string;
    url: string;
    status: string;
    downloadedBytes: number;
    totalBytes: number | null;
  }[],
): void {
  list.innerHTML = "";
  if (jobs.length === 0) {
    const empty = document.createElement("li");
    empty.className = "sdm-empty";
    empty.textContent = "No downloads yet";
    list.appendChild(empty);
    return;
  }
  for (const job of jobs.slice(0, 8)) {
    const item = document.createElement("li");
    item.className = `sdm-job sdm-job-${job.status}`;
    const name = document.createElement("span");
    name.className = "sdm-job-name";
    name.textContent = job.url.split("/").pop() || job.url;
    const status = document.createElement("span");
    status.className = "sdm-job-status";
    const pct =
      job.totalBytes && job.totalBytes > 0
        ? `${Math.round((job.downloadedBytes / job.totalBytes) * 100)}%`
        : job.status;
    status.textContent = pct;
    item.append(name, status);
    list.appendChild(item);
  }
}

function extractUrlsFromDataTransfer(dt: DataTransfer): string[] {
  const uriList = dt.getData("text/uri-list");
  const plain = dt.getData("text/plain");
  const combined = [uriList, plain].filter(Boolean).join("\n");
  return dedupeUrls(extractUrls(combined));
}

export async function initPopup(root: ParentNode = document): Promise<void> {
  const els = queryElements(root);
  const settingsStore = new ChromeStorageSettingsStore();
  const settings = await loadSettings(settingsStore);
  const client = new SdmApiClient(settings.apiBaseUrl, settings.pairingToken);

  async function refreshStatus(): Promise<void> {
    try {
      const status = await client.pairingStatus();
      els.statusDot.classList.toggle("sdm-connected", status.connected);
      els.statusDot.classList.toggle("sdm-disconnected", !status.connected);
      els.statusText.textContent = status.connected
        ? "Connected to SmartDownloadManager"
        : settings.pairingToken
          ? "Paired, but sdmd isn't responding"
          : "Not paired — open settings to pair";
    } catch {
      els.statusDot.classList.remove("sdm-connected");
      els.statusDot.classList.add("sdm-disconnected");
      els.statusText.textContent = "Can't reach sdmd — is it running?";
    }
  }

  async function refreshJobs(): Promise<void> {
    try {
      const jobs = await client.listJobs();
      renderJobs(
        els.jobsList,
        [...jobs].sort((a, b) => (a.id < b.id ? 1 : -1)),
      );
    } catch {
      // Leave whatever was last rendered — a transient failure here
      // shouldn't blank out the list the user was just looking at.
    }
  }

  async function queueUrls(urls: string[]): Promise<void> {
    const deduped = dedupeUrls(urls);
    if (deduped.length === 0) return;
    if (deduped.length === 1) {
      await client.capture({
        url: deduped[0],
        pageUrl: null,
        suggestedFilename: null,
        sizeHintBytes: null,
        source: "drag-drop",
      });
    } else {
      await client.captureBatch({ urls: deduped, pageUrl: null });
    }
    await refreshJobs();
  }

  els.queueButton.addEventListener("click", () => {
    const urls = dedupeUrls(extractUrls(els.pasteArea.value));
    if (urls.length === 0) return;
    void client.captureBatch({ urls, pageUrl: null }).then(() => {
      els.pasteArea.value = "";
      void refreshJobs();
    });
  });

  els.dropZone.addEventListener("dragover", (event) => {
    event.preventDefault();
    els.dropZone.classList.add("sdm-drag-over");
  });
  els.dropZone.addEventListener("dragleave", () => {
    els.dropZone.classList.remove("sdm-drag-over");
  });
  els.dropZone.addEventListener("drop", (event) => {
    event.preventDefault();
    els.dropZone.classList.remove("sdm-drag-over");
    if (!event.dataTransfer) return;
    const urls = extractUrlsFromDataTransfer(event.dataTransfer);
    void queueUrls(urls);
  });

  els.optionsLink.addEventListener("click", (event) => {
    event.preventDefault();
    const api =
      (globalThis as typeof globalThis & { chrome?: typeof chrome; browser?: typeof chrome })
        .chrome ?? (globalThis as typeof globalThis & { browser?: typeof chrome }).browser;
    api?.runtime.openOptionsPage();
  });

  await refreshStatus();
  await refreshJobs();
  // Live-ish polling while the popup is open — closing the popup tears
  // this down automatically since the interval lives in the popup's own
  // (short-lived) document.
  const interval = setInterval(() => {
    void refreshStatus();
    void refreshJobs();
  }, 2000);
  window.addEventListener("unload", () => clearInterval(interval));
}
