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
