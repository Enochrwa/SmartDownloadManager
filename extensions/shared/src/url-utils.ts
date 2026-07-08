/**
 * Pure, framework-free URL helpers shared by every capture path (batch
 * paste, drag & drop, clipboard monitoring, download interception). Kept
 * dependency-free and side-effect-free so they're trivially unit-testable
 * without any browser extension APIs — see `url-utils.test.ts`.
 */

const URL_PATTERN = /\bhttps?:\/\/[^\s<>"'\)\]]+/gi;

/**
 * Extract every `http(s)://` URL from a block of free-form text (e.g.
 * something pasted into the popup, per Sprint 11's "batch URL detection"
 * scope note). Trailing punctuation commonly glued onto a URL in prose
 * (a period, comma, or closing bracket at the very end) is trimmed off
 * since it's essentially never part of the URL itself.
 */
export function extractUrls(text: string): string[] {
  const matches = text.match(URL_PATTERN) ?? [];
  return matches.map(trimTrailingPunctuation).filter((u) => u.length > 0);
}

const TRAILING_PUNCTUATION = /[.,;:!?)\]}>'"]+$/;

function trimTrailingPunctuation(url: string): string {
  return url.replace(TRAILING_PUNCTUATION, "");
}

/**
 * Case-preserving de-duplication that treats two URLs as "the same" if
 * they're byte-identical after trimming a trailing slash — good enough
 * for "don't queue the same pasted link twice" without being a full URL
 * normalizer (that job belongs to the engine's own duplicate detection,
 * which additionally considers destination filename and checksum).
 */
export function dedupeUrls(urls: string[]): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const url of urls) {
    const key = normalizeForDedupe(url);
    if (!seen.has(key)) {
      seen.add(key);
      out.push(url);
    }
  }
  return out;
}

function normalizeForDedupe(url: string): string {
  return url.trim().replace(/\/+$/, "");
}

/** File extensions Sprint 11's auto-detect scope note calls out by name. */
const AUTO_DETECT_EXTENSIONS = new Set([
  "mp4",
  "mkv",
  "avi",
  "mov",
  "webm",
  "mp3",
  "flac",
  "wav",
  "aac",
  "ogg",
  "pdf",
  "zip",
  "rar",
  "7z",
  "tar",
  "gz",
]);

/** Best-effort file extension from a URL's path, ignoring query/fragment. */
export function extensionOf(url: string): string | null {
  try {
    const { pathname } = new URL(url);
    const last = pathname.split("/").pop() ?? "";
    const dot = last.lastIndexOf(".");
    if (dot === -1 || dot === last.length - 1) return null;
    return last.slice(dot + 1).toLowerCase();
  } catch {
    return null;
  }
}

/** Whether a URL's extension matches Sprint 11's built-in auto-detect list. */
export function isAutoDetectableMedia(url: string): boolean {
  const ext = extensionOf(url);
  return ext !== null && AUTO_DETECT_EXTENSIONS.has(ext);
}

export interface InterceptionSettings {
  /** Master on/off switch for download interception entirely. */
  interceptEnabled: boolean;
  /** Only intercept downloads at or above this size; 0 disables the check. */
  minSizeBytes: number;
  /** Lower-cased extensions (no dot) that are always intercepted regardless of size, e.g. ["mp4","zip"]. */
  fileTypeAllowlist: string[];
  /** Hostnames the user has explicitly opted in for on this site, regardless of size/type. */
  perSiteOptIn: string[];
}

/**
 * The core "should sdmd take this download instead of the browser?"
 * decision (Sprint 11: "by size threshold, file-type allowlist, or user
 * opt-in per-site"). Pulled out of `background.ts` so it's unit-testable
 * without any `chrome.downloads` mocking.
 */
export function shouldIntercept(
  url: string,
  sizeBytes: number | null,
  settings: InterceptionSettings,
): boolean {
  if (!settings.interceptEnabled) return false;

  let hostname: string | null = null;
  try {
    hostname = new URL(url).hostname;
  } catch {
    hostname = null;
  }
  if (hostname && settings.perSiteOptIn.includes(hostname)) {
    return true;
  }

  const ext = extensionOf(url);
  if (ext && settings.fileTypeAllowlist.includes(ext)) {
    return true;
  }

  if (settings.minSizeBytes > 0 && sizeBytes !== null && sizeBytes >= settings.minSizeBytes) {
    return true;
  }

  return false;
}
