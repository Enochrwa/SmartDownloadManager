import type { Job, JobEvent } from "@sdm/common-types";
import { beforeEach, describe, expect, it } from "vitest";
import { useQueueStore } from "./store";

const baseJob: Job = {
  id: "job-1",
  url: "https://example.com/file.zip",
  destination: "/downloads/file.zip",
  status: "queued",
  downloadedBytes: 0,
  totalBytes: 1000,
  connections: 1,
  checksumVerified: false,
};

beforeEach(() => {
  useQueueStore.setState({ jobs: {}, speedHistory: {}, theme: "light" });
});

describe("useQueueStore", () => {
  it("sets jobs from a list", () => {
    useQueueStore.getState().setJobs([baseJob]);
    expect(useQueueStore.getState().jobs["job-1"]).toEqual(baseJob);
  });

  it("upserts a single job", () => {
    useQueueStore.getState().upsertJob(baseJob);
    expect(Object.keys(useQueueStore.getState().jobs)).toEqual(["job-1"]);
  });

  it("removes a job and its speed history", () => {
    useQueueStore.getState().upsertJob(baseJob);
    useQueueStore.getState().applyEvent({
      type: "progress",
      jobId: "job-1",
      downloadedBytes: 100,
      totalBytes: 1000,
      speedBps: 512,
    });
    expect(useQueueStore.getState().speedHistory["job-1"]).toHaveLength(1);

    useQueueStore.getState().removeJob("job-1");
    expect(useQueueStore.getState().jobs["job-1"]).toBeUndefined();
    expect(useQueueStore.getState().speedHistory["job-1"]).toBeUndefined();
  });

  it("applies a progress event, updating bytes and appending a speed sample", () => {
    useQueueStore.getState().upsertJob(baseJob);
    const event: JobEvent = {
      type: "progress",
      jobId: "job-1",
      downloadedBytes: 250,
      totalBytes: 1000,
      speedBps: 2048,
    };
    useQueueStore.getState().applyEvent(event);

    const job = useQueueStore.getState().jobs["job-1"];
    expect(job.downloadedBytes).toBe(250);
    expect(job.status).toBe("downloading");
    expect(useQueueStore.getState().speedHistory["job-1"]).toEqual([
      expect.objectContaining({ bps: 2048 }),
    ]);
  });

  it("applies a completed event, filling in final byte counts", () => {
    useQueueStore.getState().upsertJob(baseJob);
    useQueueStore.getState().applyEvent({
      type: "completed",
      jobId: "job-1",
      destination: "/downloads/file.zip",
      totalBytes: 1000,
    });
    const job = useQueueStore.getState().jobs["job-1"];
    expect(job.status).toBe("completed");
    expect(job.downloadedBytes).toBe(1000);
  });

  it("applies a failed event, recording the error message", () => {
    useQueueStore.getState().upsertJob(baseJob);
    useQueueStore.getState().applyEvent({
      type: "failed",
      jobId: "job-1",
      errorClass: "timeout",
      message: "Connection timed out",
    });
    const job = useQueueStore.getState().jobs["job-1"];
    expect(job.status).toBe("failed");
    expect(job.errorMessage).toBe("Connection timed out");
  });

  it("creates a job entry from an event when none existed yet (e.g. auto-resume)", () => {
    useQueueStore.getState().applyEvent({ type: "probing", jobId: "job-2" });
    expect(useQueueStore.getState().jobs["job-2"]).toBeDefined();
    expect(useQueueStore.getState().jobs["job-2"].status).toBe("probing");
  });

  it("caps speed history at 60 samples", () => {
    useQueueStore.getState().upsertJob(baseJob);
    for (let i = 0; i < 70; i += 1) {
      useQueueStore.getState().applyEvent({
        type: "progress",
        jobId: "job-1",
        downloadedBytes: i,
        totalBytes: 1000,
        speedBps: i,
      });
    }
    expect(useQueueStore.getState().speedHistory["job-1"].length).toBeLessThanOrEqual(60);
  });

  it("sets the theme", () => {
    useQueueStore.getState().setTheme("dark");
    expect(useQueueStore.getState().theme).toBe("dark");
  });
});
