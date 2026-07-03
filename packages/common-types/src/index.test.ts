import { describe, expect, it } from "vitest";
import type { Job, JobEvent, JobStatus } from "./index";

describe("Job type", () => {
  it("accepts a well-formed Job", () => {
    const status: JobStatus = "downloading";
    const job: Job = {
      id: "abc123",
      url: "https://example.com/file.zip",
      destination: "/downloads/file.zip",
      status,
      downloadedBytes: 1024,
      totalBytes: 2048,
      connections: 4,
      checksumVerified: false,
    };
    expect(job.status).toBe("downloading");
    expect(job.connections).toBe(4);
  });
});

describe("JobEvent type", () => {
  it("discriminates on type for a progress event", () => {
    const event: JobEvent = {
      type: "progress",
      jobId: "abc123",
      downloadedBytes: 512,
      totalBytes: 2048,
      speedBps: 1024,
    };
    if (event.type === "progress") {
      expect(event.speedBps).toBe(1024);
    } else {
      throw new Error("expected a progress event");
    }
  });

  it("discriminates on type for a completed event", () => {
    const event: JobEvent = {
      type: "completed",
      jobId: "abc123",
      destination: "/downloads/file.zip",
      totalBytes: 2048,
    };
    expect(event.type).toBe("completed");
  });
});
