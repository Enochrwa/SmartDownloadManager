import type { MediaFormat, MediaProbeResult } from "@sdm/common-types";
import { useEffect, useRef, useState } from "react";
import { api } from "../api";

interface AddDownloadDialogProps {
  open: boolean;
  defaultDir: string;
  onClose: () => void;
}

/**
 * Cheap client-side mirror of `sdm_engine::looks_like_media_url`'s known
 * host list — good enough to decide, as the person types, whether to
 * show the "video/audio link detected" banner and kick off a
 * `probeMedia` call. This is a UX nicety only: the actual download still
 * goes through the backend's own `detect_media_source` (known hosts +
 * live yt-dlp probe fallback), so a host missing from this shortlist
 * still gets captured correctly — it just won't show the quality picker
 * before the download starts.
 */
const KNOWN_MEDIA_HOSTS = [
  "youtube.com",
  "youtu.be",
  "vimeo.com",
  "dailymotion.com",
  "twitch.tv",
  "soundcloud.com",
  "tiktok.com",
  "x.com/",
  "twitter.com/",
  "facebook.com/watch",
  "fb.watch",
  "instagram.com/reel",
  "instagram.com/p/",
  "instagram.com/tv/",
  "reddit.com/r/",
  "v.redd.it",
  "streamable.com",
  "bilibili.com",
  "bandcamp.com",
  "rumble.com",
];

function looksLikeMediaUrl(url: string): boolean {
  const lower = url.toLowerCase();
  return KNOWN_MEDIA_HOSTS.some((h) => lower.includes(h));
}

function formatBytes(n?: number | null): string {
  if (!n) return "";
  const units = ["B", "KB", "MB", "GB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v < 10 && i > 0 ? 1 : 0)} ${units[i]}`;
}

function formatDuration(seconds?: number | null): string {
  if (!seconds) return "";
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

export function AddDownloadDialog({ open, defaultDir, onClose }: AddDownloadDialogProps) {
  const [url, setUrl] = useState("");
  const [destination, setDestination] = useState("");
  const [connections, setConnections] = useState("auto");
  const [checksum, setChecksum] = useState("");
  const [onDuplicate, setOnDuplicate] = useState("rename");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // "Capture any link" media state.
  const [captureMedia, setCaptureMedia] = useState(true);
  const [probe, setProbe] = useState<MediaProbeResult | null>(null);
  const [probing, setProbing] = useState(false);
  const [quality, setQuality] = useState("best");
  const [embedThumbnail, setEmbedThumbnail] = useState(true);
  const probeTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const isMediaCandidate = looksLikeMediaUrl(url);

  useEffect(() => {
    clearTimeout(probeTimer.current);
    setProbe(null);
    if (!isMediaCandidate || !url.trim()) return;
    // Debounce so we don't fire a yt-dlp probe (a real subprocess) on
    // every keystroke — only after the person pauses typing.
    probeTimer.current = setTimeout(async () => {
      setProbing(true);
      try {
        const result = await api.probeMedia(url.trim());
        setProbe(result);
      } catch {
        // A probe failure just means no quality picker — the download
        // itself still falls back to the backend's own detection, so
        // this is never fatal to "Add download".
        setProbe(null);
      } finally {
        setProbing(false);
      }
    }, 600);
    return () => clearTimeout(probeTimer.current);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [url, isMediaCandidate]);

  if (!open) return null;

  const reset = () => {
    setUrl("");
    setDestination("");
    setConnections("auto");
    setChecksum("");
    setOnDuplicate("rename");
    setError(null);
    setCaptureMedia(true);
    setProbe(null);
    setQuality("best");
    setEmbedThumbnail(true);
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!url.trim()) {
      setError("Paste a URL to download.");
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      await api.addDownload({
        url: url.trim(),
        destination: destination.trim() || undefined,
        connections,
        checksum: checksum.trim() || undefined,
        onDuplicate,
        forceMedia: isMediaCandidate ? captureMedia : undefined,
        mediaQuality: isMediaCandidate && captureMedia ? quality : undefined,
        embedThumbnail: isMediaCandidate && captureMedia ? embedThumbnail : undefined,
      });
      reset();
      onClose();
    } catch (err) {
      setError(String(err));
    } finally {
      setSubmitting(false);
    }
  };

  const showMediaPanel = isMediaCandidate && captureMedia;

  return (
    <div
      className="sdm-modal-backdrop"
      onClick={onClose}
      onKeyDown={(e) => e.key === "Escape" && onClose()}
    >
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: stopPropagation-only handler, not a standalone interactive action. */}
      <form
        className="sdm-modal"
        onClick={(e) => e.stopPropagation()}
        onSubmit={handleSubmit}
        aria-label="Add download"
      >
        <h2>Add download</h2>
        <p className="sdm-modal-subtitle">
          Paste any link — a direct file, or a page on YouTube, TikTok, Vimeo, and thousands of
          other sites. SmartDownloadManager extracts the video or audio automatically.
        </p>

        <label htmlFor="sdm-url">URL</label>
        <input
          id="sdm-url"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          placeholder="https://example.com/file.zip or https://youtu.be/…"
        />

        {isMediaCandidate && (
          <div className="sdm-media-banner">
            <label className="sdm-checkbox-row">
              <input
                type="checkbox"
                checked={captureMedia}
                onChange={(e) => setCaptureMedia(e.target.checked)}
              />
              <span>🎬 Video/audio link detected — extract with yt-dlp</span>
            </label>

            {captureMedia && (
              <div className="sdm-media-details">
                {probing && <p className="sdm-media-status">Fetching title and formats…</p>}
                {probe?.isPlaylist && (
                  <p className="sdm-media-status">{probe.title ?? "Playlist detected"}</p>
                )}
                {probe && !probe.isPlaylist && (
                  <div className="sdm-media-preview">
                    {probe.thumbnail && (
                      <img src={probe.thumbnail} alt="" className="sdm-media-thumb" />
                    )}
                    <div>
                      {probe.title && <p className="sdm-media-title">{probe.title}</p>}
                      {probe.durationSeconds != null && (
                        <p className="sdm-media-meta">{formatDuration(probe.durationSeconds)}</p>
                      )}
                    </div>
                  </div>
                )}

                <label htmlFor="sdm-quality">Quality</label>
                <select
                  id="sdm-quality"
                  value={quality}
                  onChange={(e) => setQuality(e.target.value)}
                >
                  <option value="best">Best available</option>
                  {probe?.formats
                    .filter((f: MediaFormat) => f.hasVideo || f.hasAudio)
                    .map((f: MediaFormat) => (
                      <option key={f.formatId} value={f.formatId}>
                        {f.qualityLabel}
                        {f.filesizeBytes ? ` — ${formatBytes(f.filesizeBytes)}` : ""}
                      </option>
                    ))}
                </select>

                <label className="sdm-checkbox-row">
                  <input
                    type="checkbox"
                    checked={embedThumbnail}
                    onChange={(e) => setEmbedThumbnail(e.target.checked)}
                  />
                  <span>Embed thumbnail as cover art</span>
                </label>
              </div>
            )}
          </div>
        )}

        <label htmlFor="sdm-destination">Save to (optional)</label>
        <input
          id="sdm-destination"
          value={destination}
          onChange={(e) => setDestination(e.target.value)}
          placeholder={defaultDir ? `${defaultDir}/…` : "Default download folder"}
        />

        {!showMediaPanel && (
          <div className="sdm-form-row">
            <div>
              <label htmlFor="sdm-connections">Connections</label>
              <select
                id="sdm-connections"
                value={connections}
                onChange={(e) => setConnections(e.target.value)}
              >
                <option value="auto">Auto</option>
                {[1, 2, 4, 8, 16, 32].map((n) => (
                  <option key={n} value={String(n)}>
                    {n}
                  </option>
                ))}
              </select>
            </div>
            <div>
              <label htmlFor="sdm-on-duplicate">If duplicate</label>
              <select
                id="sdm-on-duplicate"
                value={onDuplicate}
                onChange={(e) => setOnDuplicate(e.target.value)}
              >
                <option value="rename">Rename</option>
                <option value="overwrite">Overwrite</option>
                <option value="skip">Skip</option>
              </select>
            </div>
          </div>
        )}

        {!showMediaPanel && (
          <>
            <label htmlFor="sdm-checksum">Expected checksum (optional)</label>
            <input
              id="sdm-checksum"
              value={checksum}
              onChange={(e) => setChecksum(e.target.value)}
              placeholder="sha256:abcd1234…"
            />
          </>
        )}

        {error && (
          <p className="sdm-form-error" role="alert">
            {error}
          </p>
        )}

        <div className="sdm-modal-actions">
          <button type="button" onClick={onClose} disabled={submitting}>
            Cancel
          </button>
          <button type="submit" className="sdm-primary" disabled={submitting}>
            {submitting ? "Adding…" : "Add download"}
          </button>
        </div>
      </form>
    </div>
  );
}
