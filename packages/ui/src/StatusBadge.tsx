import type { JobStatus } from "@sdm/common-types";

const LABELS: Record<JobStatus, string> = {
  queued: "Queued",
  probing: "Probing",
  downloading: "Downloading",
  paused: "Paused",
  verifying: "Verifying",
  completed: "Completed",
  failed: "Failed",
};

const COLORS: Record<JobStatus, string> = {
  queued: "var(--sdm-status-neutral)",
  probing: "var(--sdm-status-info)",
  downloading: "var(--sdm-status-info)",
  paused: "var(--sdm-status-warning)",
  verifying: "var(--sdm-status-info)",
  completed: "var(--sdm-status-success)",
  failed: "var(--sdm-status-danger)",
};

export function StatusBadge({ status }: { status: JobStatus }) {
  return (
    <span
      className="sdm-status-badge"
      style={{
        color: COLORS[status],
        borderColor: COLORS[status],
      }}
      data-status={status}
    >
      {LABELS[status]}
    </span>
  );
}
