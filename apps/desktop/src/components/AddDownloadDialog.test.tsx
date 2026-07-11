import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { AddDownloadDialog } from "./AddDownloadDialog";

const mockApi = vi.hoisted(() => ({
  addDownload: vi.fn(),
  probeMedia: vi.fn(),
}));

vi.mock("../api", () => ({ api: mockApi }));

beforeEach(() => {
  vi.clearAllMocks();
});

describe("AddDownloadDialog", () => {
  it("renders nothing when closed", () => {
    const { container } = render(
      <AddDownloadDialog open={false} defaultDir="/downloads" onClose={() => {}} />,
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("shows a validation error when submitting without a URL", async () => {
    render(<AddDownloadDialog open defaultDir="/downloads" onClose={() => {}} />);
    fireEvent.click(screen.getByRole("button", { name: "Add download" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Paste a URL");
    expect(mockApi.addDownload).not.toHaveBeenCalled();
  });

  it("calls onClose after a successful submit", async () => {
    mockApi.addDownload.mockResolvedValue(undefined);
    const onClose = vi.fn();
    render(<AddDownloadDialog open defaultDir="/downloads" onClose={onClose} />);

    fireEvent.change(
      screen.getByPlaceholderText("https://example.com/file.zip or https://youtu.be/…"),
      { target: { value: "https://example.com/movie.mkv" } },
    );
    fireEvent.click(screen.getByRole("button", { name: "Add download" }));

    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(mockApi.addDownload).toHaveBeenCalledWith(
      expect.objectContaining({ url: "https://example.com/movie.mkv", connections: "auto" }),
    );
  });

  it("surfaces a backend error instead of closing", async () => {
    mockApi.addDownload.mockRejectedValue(new Error("duplicate job"));
    const onClose = vi.fn();
    render(<AddDownloadDialog open defaultDir="/downloads" onClose={onClose} />);

    fireEvent.change(
      screen.getByPlaceholderText("https://example.com/file.zip or https://youtu.be/…"),
      {
        target: { value: "https://example.com/movie.mkv" },
      },
    );
    fireEvent.click(screen.getByRole("button", { name: "Add download" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("duplicate job");
    expect(onClose).not.toHaveBeenCalled();
  });

  it("shows a capture-any-link banner and quality picker for a known video host", async () => {
    mockApi.probeMedia.mockResolvedValue({
      title: "Big Buck Bunny",
      thumbnail: null,
      durationSeconds: 596,
      isLivestream: false,
      isPlaylist: false,
      formats: [
        { formatId: "best", qualityLabel: "1080p", hasVideo: true, hasAudio: true },
        { formatId: "worst", qualityLabel: "360p", hasVideo: true, hasAudio: true },
      ],
    });
    render(<AddDownloadDialog open defaultDir="/downloads" onClose={() => {}} />);

    fireEvent.change(
      screen.getByPlaceholderText("https://example.com/file.zip or https://youtu.be/…"),
      { target: { value: "https://youtu.be/6hkG4-LgwvI" } },
    );

    expect(
      await screen.findByText("🎬 Video/audio link detected — extract with yt-dlp"),
    ).toBeInTheDocument();
  });

  it("passes forceMedia and quality through to addDownload for a video link", async () => {
    mockApi.addDownload.mockResolvedValue(undefined);
    mockApi.probeMedia.mockResolvedValue({
      isLivestream: false,
      isPlaylist: false,
      formats: [],
    });
    const onClose = vi.fn();
    render(<AddDownloadDialog open defaultDir="/downloads" onClose={onClose} />);

    fireEvent.change(
      screen.getByPlaceholderText("https://example.com/file.zip or https://youtu.be/…"),
      { target: { value: "https://youtu.be/6hkG4-LgwvI" } },
    );
    fireEvent.click(screen.getByRole("button", { name: "Add download" }));

    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(mockApi.addDownload).toHaveBeenCalledWith(
      expect.objectContaining({
        url: "https://youtu.be/6hkG4-LgwvI",
        forceMedia: true,
        mediaQuality: "best",
      }),
    );
  });
});
