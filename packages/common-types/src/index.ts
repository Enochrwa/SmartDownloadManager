// Shared TypeScript types mirroring the Rust API DTOs (crates/api).
// Kept hand-in-sync until Sprint 5 introduces OpenAPI-generated types.

export type JobStatus =
  | "Queued"
  | "Downloading"
  | "Paused"
  | "Verifying"
  | "Completed"
  | "Failed";

export interface Job {
  id: string;
  url: string;
  destination: string;
  status: JobStatus;
  totalBytes?: number;
  downloadedBytes: number;
}
