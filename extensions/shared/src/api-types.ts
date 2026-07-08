/**
 * Hand-maintained TypeScript mirror of `crates/api-types/src/lib.rs`.
 *
 * The browser extension has no Cargo access and no code-generation step
 * wired up (yet — see docs/SPRINT_PLAN_PHASE2.md Sprint 11 scope notes),
 * so these shapes are kept in sync by hand. Any field added to the Rust
 * DTOs that the extension consumes or sends should be mirrored here in
 * the same PR.
 */

export interface JobResponse {
  id: string;
  url: string;
  destination: string;
  status: string;
  jobKind: string;
  downloadedBytes: number;
  totalBytes: number | null;
  connections: number;
  errorClass: string | null;
  errorMessage: string | null;
  parentJobId: string | null;
}

/** One of "download-intercept" | "context-menu" | "clipboard" | "drag-drop" | "batch-paste". */
export type CaptureSource =
  | "download-intercept"
  | "context-menu"
  | "clipboard"
  | "drag-drop"
  | "batch-paste";

export interface CaptureRequest {
  url: string;
  pageUrl: string | null;
  suggestedFilename: string | null;
  sizeHintBytes: number | null;
  source: CaptureSource;
}

export interface CaptureResponse {
  job: JobResponse;
  deduplicated: boolean;
}

export interface BatchCaptureRequest {
  urls: string[];
  pageUrl: string | null;
}

export interface BatchCaptureResult {
  url: string;
  job: JobResponse | null;
  deduplicated: boolean;
  error: string | null;
}

export interface BatchCaptureResponse {
  results: BatchCaptureResult[];
}

export interface PairingTokenIssueResponse {
  token: string;
  label: string;
  createdAt: string;
}

export interface PairingVerifyResponse {
  ok: boolean;
}

export interface PairedExtensionInfo {
  label: string;
  createdAt: string;
  lastSeenAt: string | null;
}

export interface PairingStatusResponse {
  connected: boolean;
  pairedExtensions: PairedExtensionInfo[];
}

export interface ErrorResponse {
  error: string;
}

/**
 * The server's JSON is snake_case (serde default); the extension's
 * TypeScript types above are camelCase for consistency with the rest of
 * the TS codebase. `fromServerJson`/`toServerJson` do the field-name
 * translation at the network boundary so the rest of the extension never
 * has to think in snake_case.
 */
export function snakeToCamel(input: unknown): unknown {
  if (Array.isArray(input)) {
    return input.map(snakeToCamel);
  }
  if (input !== null && typeof input === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(input as Record<string, unknown>)) {
      const camelKey = key.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());
      out[camelKey] = snakeToCamel(value);
    }
    return out;
  }
  return input;
}

export function camelToSnake(input: unknown): unknown {
  if (Array.isArray(input)) {
    return input.map(camelToSnake);
  }
  if (input !== null && typeof input === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(input as Record<string, unknown>)) {
      const snakeKey = key.replace(/[A-Z]/g, (c: string) => `_${c.toLowerCase()}`);
      out[snakeKey] = camelToSnake(value);
    }
    return out;
  }
  return input;
}
