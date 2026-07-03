import type { Job, JobEvent, RepairReport } from "@sdm/common-types";
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
  }): Promise<void> {
    return invoke("add_download", args);
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
  removeJob(jobId: string): Promise<void> {
    return invoke("remove_job", { jobId });
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
  onJobEvent(handler: (event: JobEvent) => void): Promise<UnlistenFn> {
    return listen<JobEvent>("job-event", (e) => handler(e.payload));
  },
};
