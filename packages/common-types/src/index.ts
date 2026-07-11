// Shared TypeScript types mirroring the Rust API DTOs.
//
// `Job`/`JobEvent` are hand-in-sync with `apps/desktop/src-tauri/src/dto.rs`
// (Sprint 6). Kept hand-in-sync until a later sprint introduces
// OpenAPI/ts-rs-generated types shared with `crates/api-types`.

export type JobStatus =
  | "queued"
  | "probing"
  | "downloading"
  | "paused"
  | "verifying"
  | "completed"
  | "failed";

export interface Job {
  id: string;
  url: string;
  destination: string;
  status: JobStatus;
  totalBytes?: number | null;
  downloadedBytes: number;
  connections: number;
  checksumAlgorithm?: string | null;
  checksumActual?: string | null;
  checksumVerified: boolean;
  errorMessage?: string | null;
  /** "http" | "ftp" | "torrent" | "sftp" | "scp" | "webdav" | "hls" |
   * "dash" | "media" — "media" jobs came from the "capture any link"
   * yt-dlp/FFmpeg path (Sprint 10) rather than a direct file transfer.
   * Optional only so existing test fixtures/older cached rows without it
   * still type-check; treat a missing value as `"http"`. */
  jobKind?: string;
  parentJobId?: string | null;
  /** Populated for `jobKind === "media"` jobs once probed. */
  mediaTitle?: string | null;
  mediaThumbnail?: string | null;
}

/** One selectable quality/codec format from `probeMedia` — backs the Add
 * Download dialog's quality picker for a "capture any link" URL. */
export interface MediaFormat {
  formatId: string;
  qualityLabel: string;
  ext?: string | null;
  hasVideo: boolean;
  hasAudio: boolean;
  filesizeBytes?: number | null;
}

export interface MediaProbeResult {
  title?: string | null;
  thumbnail?: string | null;
  durationSeconds?: number | null;
  isLivestream: boolean;
  isPlaylist: boolean;
  formats: MediaFormat[];
}

/** Streamed from the Tauri backend on the `job-event` event channel. */
export type JobEvent =
  | { type: "queued"; jobId: string }
  | { type: "probing"; jobId: string }
  | { type: "started"; jobId: string; totalBytes?: number | null; connections: number }
  | {
      type: "progress";
      jobId: string;
      downloadedBytes: number;
      totalBytes?: number | null;
      speedBps: number;
    }
  | { type: "verifying"; jobId: string }
  | { type: "retrying"; jobId: string; errorClass: string; attempt: number; delayMs: number }
  | { type: "completed"; jobId: string; destination: string; totalBytes: number }
  | { type: "failed"; jobId: string; errorClass: string; message: string }
  | { type: "paused"; jobId: string };

export interface RepairReport {
  integrityErrors: string[];
  action: "none_needed" | "restored_from_backup" | "recreated_empty";
  detail?: string | null;
}

/** One in-memory speed sample used to render the live speed graph. */
export interface SpeedSample {
  t: number; // ms since epoch
  bps: number;
}

/**
 * Sprint 11: the browser-extension pairing flow. Hand-in-sync with
 * `apps/desktop/src-tauri/src/dto.rs`'s `PairingTokenDto`/`PairingStatusDto`.
 */
export interface PairingToken {
  token: string;
  label: string;
  createdAt: string;
}

export interface PairedExtension {
  label: string;
  createdAt: string;
  lastSeenAt?: string | null;
}

export interface PairingStatus {
  connected: boolean;
  pairedExtensions: PairedExtension[];
  /**
   * The port the embedded extension API is actually bound to right now.
   * `null`/absent while it's still starting or if it failed to bind at
   * all this session — see `apiError`. Sprint 12 fix: this used to
   * always report the configured constant regardless of whether the
   * bind succeeded, which is what produced "Couldn't reach sdmd at this
   * address" with no visible cause in the UI.
   */
  apiPort: number | null;
  /** Set when `apiPort` is null and the cause is a real failure (as
   * opposed to just "still starting"), or a human-readable status while
   * starting. Shown directly in the pairing panel. */
  apiError?: string | null;
}
