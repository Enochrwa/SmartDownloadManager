import { create } from "zustand";
import type { Job } from "@sdm/common-types";

/**
 * Client-side queue/job state, fed by the sdmd WebSocket event stream
 * (Sprint 5+, see docs/SPRINT_PLAN.md). Zustand over useState/Redux because
 * this state updates frequently from a stream rather than from request/
 * response — see docs/TECH_DECISIONS.md §3 for the reasoning.
 */
interface QueueState {
  jobs: Record<string, Job>;
  upsertJob: (job: Job) => void;
  removeJob: (id: string) => void;
}

export const useQueueStore = create<QueueState>((set) => ({
  jobs: {},
  upsertJob: (job) =>
    set((state) => ({ jobs: { ...state.jobs, [job.id]: job } })),
  removeJob: (id) =>
    set((state) => {
      const { [id]: _, ...rest } = state.jobs;
      return { jobs: rest };
    }),
}));
