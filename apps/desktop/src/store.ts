import type { Job, JobEvent, JobStatus, SpeedSample } from "@sdm/common-types";
import { create } from "zustand";

/** How many speed samples to keep per job for the live speed graph. */
const MAX_SPEED_SAMPLES = 60;

export type Theme = "light" | "dark";

interface QueueState {
  jobs: Record<string, Job>;
  /** Recent speed samples per job, newest last — feeds the sparkline. */
  speedHistory: Record<string, SpeedSample[]>;
  theme: Theme;
  setTheme: (theme: Theme) => void;
  setJobs: (jobs: Job[]) => void;
  upsertJob: (job: Job) => void;
  removeJob: (id: string) => void;
  applyEvent: (event: JobEvent) => void;
}

function statusForEvent(event: JobEvent): JobStatus | null {
  switch (event.type) {
    case "queued":
      return "queued";
    case "probing":
      return "probing";
    case "started":
      return "downloading";
    case "progress":
      return "downloading";
    case "verifying":
      return "verifying";
    case "completed":
      return "completed";
    case "failed":
      return "failed";
    case "paused":
      return "paused";
    default:
      return null;
  }
}

export const useQueueStore = create<QueueState>((set) => ({
  jobs: {},
  speedHistory: {},
  theme: "light",
  setTheme: (theme) => set({ theme }),
  setJobs: (jobs) =>
    set(() => ({
      jobs: Object.fromEntries(jobs.map((job) => [job.id, job])),
    })),
  upsertJob: (job) => set((state) => ({ jobs: { ...state.jobs, [job.id]: job } })),
  removeJob: (id) =>
    set((state) => {
      const { [id]: _removedJob, ...restJobs } = state.jobs;
      const { [id]: _removedHistory, ...restHistory } = state.speedHistory;
      return { jobs: restJobs, speedHistory: restHistory };
    }),
  applyEvent: (event) =>
    set((state) => {
      const existing = state.jobs[event.jobId];
      const status = statusForEvent(event);

      const patch: Partial<Job> = {};
      if (status) patch.status = status;
      if (event.type === "started") {
        patch.totalBytes = event.totalBytes ?? undefined;
        patch.connections = event.connections;
      }
      if (event.type === "progress") {
        patch.downloadedBytes = event.downloadedBytes;
        if (event.totalBytes != null) patch.totalBytes = event.totalBytes;
      }
      if (event.type === "completed") {
        patch.totalBytes = event.totalBytes;
        patch.downloadedBytes = event.totalBytes;
        patch.destination = event.destination;
        patch.errorMessage = null;
      }
      if (event.type === "failed") {
        patch.errorMessage = event.message;
      }

      const job: Job = existing
        ? { ...existing, ...patch }
        : {
            id: event.jobId,
            url: "",
            destination: "destination" in event ? (event.destination ?? "") : "",
            status: status ?? "queued",
            downloadedBytes: "downloadedBytes" in event ? event.downloadedBytes : 0,
            connections: "connections" in event ? event.connections : 1,
            checksumVerified: false,
            ...patch,
          };

      let speedHistory = state.speedHistory;
      if (event.type === "progress") {
        const history = state.speedHistory[event.jobId] ?? [];
        const next = [...history, { t: Date.now(), bps: event.speedBps }].slice(-MAX_SPEED_SAMPLES);
        speedHistory = { ...state.speedHistory, [event.jobId]: next };
      }

      return {
        jobs: { ...state.jobs, [event.jobId]: job },
        speedHistory,
      };
    }),
}));
