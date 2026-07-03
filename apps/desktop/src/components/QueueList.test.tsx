import type { Job } from "@sdm/common-types";
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { QueueList } from "./QueueList";

const mockApi = vi.hoisted(() => ({
  pauseJob: vi.fn(),
  resumeJob: vi.fn(),
  cancelJob: vi.fn(),
  removeJob: vi.fn(),
}));

vi.mock("../api", () => ({ api: mockApi }));

const downloadingJob: Job = {
  id: "job-1",
  url: "https://example.com/file.zip",
  destination: "/downloads/file.zip",
  status: "downloading",
  downloadedBytes: 500,
  totalBytes: 1000,
  connections: 4,
  checksumVerified: false,
};

beforeEach(() => {
  vi.clearAllMocks();
});

describe("QueueList", () => {
  it("shows the empty state with no jobs", () => {
    render(<QueueList jobs={[]} speedHistory={{}} />);
    expect(screen.getByText("No downloads yet.")).toBeInTheDocument();
  });

  it("renders job filename, status, and progress", () => {
    render(<QueueList jobs={[downloadingJob]} speedHistory={{}} />);
    expect(screen.getByText("file.zip")).toBeInTheDocument();
    expect(screen.getByText("Downloading")).toBeInTheDocument();
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "50");
  });

  it("pauses an active job", () => {
    render(<QueueList jobs={[downloadingJob]} speedHistory={{}} />);
    fireEvent.click(screen.getByText("Pause"));
    expect(mockApi.pauseJob).toHaveBeenCalledWith("job-1");
  });

  it("cancels an active job", () => {
    render(<QueueList jobs={[downloadingJob]} speedHistory={{}} />);
    fireEvent.click(screen.getByText("Cancel"));
    expect(mockApi.cancelJob).toHaveBeenCalledWith("job-1");
  });

  it("offers resume for a paused job", () => {
    render(<QueueList jobs={[{ ...downloadingJob, status: "paused" }]} speedHistory={{}} />);
    fireEvent.click(screen.getByText("Resume"));
    expect(mockApi.resumeJob).toHaveBeenCalledWith("job-1");
  });

  it("offers remove for a completed job", () => {
    render(
      <QueueList
        jobs={[{ ...downloadingJob, status: "completed", downloadedBytes: 1000 }]}
        speedHistory={{}}
      />,
    );
    fireEvent.click(screen.getByText("Remove"));
    expect(mockApi.removeJob).toHaveBeenCalledWith("job-1");
  });

  it("shows the error message for a failed job", () => {
    render(
      <QueueList
        jobs={[{ ...downloadingJob, status: "failed", errorMessage: "Connection reset" }]}
        speedHistory={{}}
      />,
    );
    expect(screen.getByText("Connection reset")).toBeInTheDocument();
  });
});
