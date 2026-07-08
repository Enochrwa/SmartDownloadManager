import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { useQueueStore } from "./store";

const mockApi = vi.hoisted(() => ({
  listJobs: vi.fn().mockResolvedValue([]),
  defaultDownloadDir: vi.fn().mockResolvedValue("/home/user/Downloads"),
  getSettings: vi.fn().mockResolvedValue({}),
  onJobEvent: vi.fn().mockResolvedValue(() => {}),
  addDownload: vi.fn().mockResolvedValue(undefined),
  setSetting: vi.fn().mockResolvedValue(undefined),
  pauseJob: vi.fn().mockResolvedValue(undefined),
  resumeJob: vi.fn().mockResolvedValue(undefined),
  cancelJob: vi.fn().mockResolvedValue(undefined),
  removeJob: vi.fn().mockResolvedValue(undefined),
  repairDatabase: vi.fn().mockResolvedValue({ integrityErrors: [], action: "none_needed" }),
  backupNow: vi.fn().mockResolvedValue("/backups/jobs-1.db"),
  cleanupOrphans: vi.fn().mockResolvedValue([]),
  pairingStatus: vi
    .fn()
    .mockResolvedValue({ connected: false, pairedExtensions: [], apiPort: 7890 }),
  pairingIssueToken: vi
    .fn()
    .mockResolvedValue({ token: "test-token", label: "Test", createdAt: "2026-01-01T00:00:00Z" }),
  pairingRevokeToken: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("./api", () => ({ api: mockApi }));

beforeEach(() => {
  vi.clearAllMocks();
  mockApi.listJobs.mockResolvedValue([]);
  mockApi.defaultDownloadDir.mockResolvedValue("/home/user/Downloads");
  mockApi.getSettings.mockResolvedValue({});
  mockApi.onJobEvent.mockResolvedValue(() => {});
  useQueueStore.setState({ jobs: {}, speedHistory: {}, theme: "light" });
});

describe("App", () => {
  it("renders the app title", async () => {
    render(<App />);
    expect(screen.getByText("SmartDownloadManager")).toBeInTheDocument();
    await waitFor(() => expect(mockApi.listJobs).toHaveBeenCalled());
  });

  it("shows the empty state when there are no jobs", async () => {
    render(<App />);
    await waitFor(() => expect(screen.getByText("No downloads yet.")).toBeInTheDocument());
  });

  it("opens the add-download dialog", async () => {
    render(<App />);
    fireEvent.click(screen.getByText("Add download"));
    expect(await screen.findByLabelText("Add download")).toBeInTheDocument();
  });

  it("submits a new download through the API", async () => {
    render(<App />);
    fireEvent.click(screen.getByText("Add download"));
    const dialog = await screen.findByLabelText("Add download");
    const urlInput = within(dialog).getByPlaceholderText("https://example.com/file.zip");
    fireEvent.change(urlInput, { target: { value: "https://example.com/movie.mkv" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Add download" }));

    await waitFor(() =>
      expect(mockApi.addDownload).toHaveBeenCalledWith(
        expect.objectContaining({ url: "https://example.com/movie.mkv" }),
      ),
    );
  });

  it("renders a job from the initial list", async () => {
    mockApi.listJobs.mockResolvedValue([
      {
        id: "job-1",
        url: "https://example.com/file.zip",
        destination: "/downloads/file.zip",
        status: "downloading",
        downloadedBytes: 512,
        totalBytes: 1024,
        connections: 4,
        checksumVerified: false,
      },
    ]);
    render(<App />);
    expect(await screen.findByText("file.zip")).toBeInTheDocument();
  });

  it("opens the settings panel", async () => {
    render(<App />);
    fireEvent.click(screen.getByLabelText("Open settings"));
    expect(await screen.findByLabelText("Settings")).toBeInTheDocument();
  });

  it("shows the browser extension pairing flow", async () => {
    render(<App />);
    fireEvent.click(screen.getByLabelText("Open settings"));
    expect(await screen.findByText("Browser Extension")).toBeInTheDocument();
    expect(await screen.findByText("No extension connected")).toBeInTheDocument();

    fireEvent.click(screen.getByText("Generate pairing token"));
    expect(await screen.findByText("test-token")).toBeInTheDocument();
    expect(mockApi.pairingIssueToken).toHaveBeenCalled();
  });
});
