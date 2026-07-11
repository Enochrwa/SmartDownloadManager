import type { Job, SpeedSample } from "@sdm/common-types";
import { ProgressBar, StatusBadge, formatBytes, formatEta, formatPercent } from "@sdm/ui";
import { useState } from "react";
import { api } from "../api";
import { SpeedGraph } from "./SpeedGraph";

interface QueueListProps {
  jobs: Job[];
  speedHistory: Record<string, SpeedSample[]>;
}

function isActive(status: Job["status"]): boolean {
  return status === "downloading" || status === "probing" || status === "verifying";
}

/**
 * Small delete control offering a real choice, the same distinction IDM
 * and JDownloader make: clear this entry from the list, or actually
 * delete the file it downloaded. Collapsed to a single "Remove" button
 * until clicked, so the common "just clear the row" case stays
 * one-click for anyone who doesn't need the extra option.
 */
function DeleteControl({ jobId, hasFile }: { jobId: string; hasFile: boolean }) {
  const [confirming, setConfirming] = useState(false);

  if (!hasFile) {
    return (
      <button type="button" onClick={() => api.removeJob(jobId, false)}>
        Remove
      </button>
    );
  }

  if (!confirming) {
    return (
      <button type="button" onClick={() => setConfirming(true)}>
        Delete
      </button>
    );
  }

  return (
    <span className="sdm-delete-confirm">
      <button
        type="button"
        onClick={() => api.removeJob(jobId, false)}
        title="Clear from list, keep the file"
      >
        Remove only
      </button>
      <button
        type="button"
        className="sdm-danger"
        onClick={() => api.removeJob(jobId, true)}
        title="Delete the downloaded file too"
      >
        Delete file
      </button>
      <button type="button" onClick={() => setConfirming(false)} aria-label="Cancel delete">
        ✕
      </button>
    </span>
  );
}

/** Icon-ish badge distinguishing "capture any link" media jobs (yt-dlp)
 * from ordinary file transfers, so a queue full of downloads is
 * scannable at a glance. */
function KindBadge({ jobKind }: { jobKind?: string }) {
  if (jobKind !== "media") return null;
  return (
    <span className="sdm-kind-badge" title="Captured with yt-dlp">
      🎬 Media
    </span>
  );
}

export function QueueList({ jobs, speedHistory }: QueueListProps) {
  if (jobs.length === 0) {
    return (
      <div className="sdm-empty-state">
        <p>No downloads yet.</p>
        <p className="sdm-muted">
          Click "Add download" and paste any link — a direct file, or a video/audio page — to get
          started.
        </p>
      </div>
    );
  }

  return (
    <ul className="sdm-queue-list">
      {jobs.map((job) => {
        const history = speedHistory[job.id] ?? [];
        const latestSpeed = history[history.length - 1]?.bps ?? 0;
        const percent = formatPercent(job.downloadedBytes, job.totalBytes);
        const isMedia = job.jobKind === "media";
        const displayName = isMedia
          ? (job.mediaTitle ?? job.destination.split("/").pop() ?? job.url)
          : job.destination.split("/").pop() || job.url;

        return (
          <li key={job.id} className="sdm-queue-row">
            {isMedia && job.mediaThumbnail && (
              <img src={job.mediaThumbnail} alt="" className="sdm-queue-thumb" />
            )}
            <div className="sdm-queue-row-main">
              <div className="sdm-queue-row-title">
                <span className="sdm-queue-filename" title={job.destination}>
                  {displayName}
                </span>
                <KindBadge jobKind={job.jobKind} />
                <StatusBadge status={job.status} />
              </div>
              <div className="sdm-queue-url" title={job.url}>
                {job.url}
              </div>
              <ProgressBar percent={percent} indeterminate={job.status === "probing"} />
              <div className="sdm-queue-meta">
                <span>
                  {formatBytes(job.downloadedBytes)} / {formatBytes(job.totalBytes)} ({percent}%)
                </span>
                <span>
                  {job.connections} connection{job.connections === 1 ? "" : "s"}
                </span>
                {isActive(job.status) && (
                  <span>ETA {formatEta(job.downloadedBytes, job.totalBytes, latestSpeed)}</span>
                )}
              </div>
              {job.errorMessage && <div className="sdm-queue-error">{job.errorMessage}</div>}
            </div>

            <div className="sdm-queue-row-side">
              {isActive(job.status) && <SpeedGraph samples={history} />}

              <div className="sdm-queue-actions">
                {isActive(job.status) && (
                  <button type="button" onClick={() => api.pauseJob(job.id)}>
                    Pause
                  </button>
                )}
                {(job.status === "paused" || job.status === "failed") && (
                  <button type="button" onClick={() => api.resumeJob(job.id)}>
                    Resume
                  </button>
                )}
                {isActive(job.status) && (
                  <button type="button" onClick={() => api.cancelJob(job.id)}>
                    Cancel
                  </button>
                )}
                {!isActive(job.status) && (
                  <DeleteControl jobId={job.id} hasFile={job.status === "completed"} />
                )}
              </div>
            </div>
          </li>
        );
      })}
    </ul>
  );
}
