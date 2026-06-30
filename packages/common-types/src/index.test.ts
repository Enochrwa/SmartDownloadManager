import { describe, expect, it } from "vitest";
import type { Job, JobStatus } from "./index";

describe("Job type", () => {
  it("accepts a well-formed Job", () => {
    const status: JobStatus = "Downloading";
    const job: Job = {
      id: "abc123",
      url: "https://example.com/file.zip",
      destination: "/downloads/file.zip",
      status,
      downloadedBytes: 1024,
    };
    expect(job.status).toBe("Downloading");
  });
});
