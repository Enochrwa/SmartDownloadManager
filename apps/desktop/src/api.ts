import type {
  Job,
  JobEvent,
  MediaProbeResult,
  PairingStatus,
  PairingToken,
  RepairReport,
} from "@sdm/common-types";
import { invoke } from "@tauri-apps/api/core";
import { type UnlistenFn, listen } from "@tauri-apps/api/event";

/**
 * All Tauri IPC calls the desktop UI makes, in one place. Components
 * import this module rather than calling `invoke`/`listen` directly, so
 * tests can mock `@sdm/desktop/api` as a whole instead of reaching into
 * `@tauri-apps/api` internals.
 */
export const api = {
  addDownload(args: {
    url: string;
    destination?: string;
    connections?: string;
    mirrors?: string[];
    checksum?: string;
    onDuplicate?: string;
    /** "Capture any link" media options (Sprint 10 + Phase-2 UI). See
     * `probeMedia` for getting a quality/format list to populate
     * `mediaQuality` from before calling this. */
    forceMedia?: boolean;
    mediaQuality?: string;
    subtitleLangs?: string[];
    embedThumbnail?: boolean;
  }): Promise<void> {
    return invoke("add_download", args);
  },
  /** Probe a URL for "capture any link" title/thumbnail/quality metadata
   * without starting a download — powers the Add Download dialog's
   * quality picker once a pasted URL is recognized as (or explicitly
   * marked as) a video/audio page. */
  probeMedia(url: string): Promise<MediaProbeResult> {
    return invoke("probe_media", { url });
  },
  resumeJob(jobId: string): Promise<void> {
    return invoke("resume_job", { jobId });
  },
  pauseJob(jobId: string): Promise<void> {
    return invoke("pause_job", { jobId });
  },
  cancelJob(jobId: string): Promise<void> {
    return invoke("cancel_job", { jobId });
  },
  /** Remove a job from the queue/history. `deleteFile: true` also
   * deletes the downloaded file(s) from disk — the real "Delete
   * download" action, not just clearing the row. */
  removeJob(jobId: string, deleteFile?: boolean): Promise<void> {
    return invoke("remove_job", { jobId, deleteFile });
  },
  listJobs(): Promise<Job[]> {
    return invoke("list_jobs");
  },
  getSettings(): Promise<Record<string, string>> {
    return invoke("get_settings");
  },
  setSetting(key: string, value: string): Promise<void> {
    return invoke("set_setting", { key, value });
  },
  defaultDownloadDir(): Promise<string> {
    return invoke("default_download_dir");
  },
  repairDatabase(): Promise<RepairReport> {
    return invoke("repair_database");
  },
  backupNow(): Promise<string> {
    return invoke("backup_now");
  },
  cleanupOrphans(deleteFiles: boolean): Promise<string[]> {
    return invoke("cleanup_orphans", { delete: deleteFiles });
  },
  /** Sprint 11: polled by the "Extension connected" indicator. */
  pairingStatus(): Promise<PairingStatus> {
    return invoke("pairing_status");
  },
  /** Sprint 11: mints a token for the first-run pairing flow. */
  pairingIssueToken(label?: string): Promise<PairingToken> {
    return invoke("pairing_issue_token", { label });
  },
  pairingRevokeToken(token: string): Promise<void> {
    return invoke("pairing_revoke_token", { token });
  },
  onJobEvent(handler: (event: JobEvent) => void): Promise<UnlistenFn> {
    return listen<JobEvent>("job-event", (e) => handler(e.payload));
  },
};
