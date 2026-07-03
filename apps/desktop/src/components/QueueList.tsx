import type { Job, SpeedSample } from "@sdm/common-types";
import { ProgressBar, StatusBadge, formatBytes, formatEta, formatPercent } from "@sdm/ui";
import { api } from "../api";
import { SpeedGraph } from "./SpeedGraph";

interface QueueListProps {
  jobs: Job[];
  speedHistory: Record<string, SpeedSample[]>;
}

function isActive(status: Job["status"]): boolean {
  return status === "downloading" || status === "probing" || status === "verifying";
}

export function QueueList({ jobs, speedHistory }: QueueListProps) {
  if (jobs.length === 0) {
    return (
      <div className="sdm-empty-state">
        <p>No downloads yet.</p>
        <p className="sdm-muted">Click "Add download" to get started.</p>
      </div>
    );
  }

  return (
    <ul className="sdm-queue-list">
      {jobs.map((job) => {
        const history = speedHistory[job.id] ?? [];
        const latestSpeed = history[history.length - 1]?.bps ?? 0;
        const percent = formatPercent(job.downloadedBytes, job.totalBytes);

        return (
          <li key={job.id} className="sdm-queue-row">
            <div className="sdm-queue-row-main">
              <div className="sdm-queue-row-title">
                <span className="sdm-queue-filename" title={job.destination}>
                  {job.destination.split("/").pop() || job.url}
                </span>
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
                  <button type="button" onClick={() => api.removeJob(job.id)}>
                    Remove
                  </button>
                )}
              </div>
            </div>
          </li>
        );
      })}
    </ul>
  );
}
