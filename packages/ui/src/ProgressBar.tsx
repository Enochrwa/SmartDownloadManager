export function ProgressBar({
  percent,
  indeterminate = false,
}: {
  percent: number;
  indeterminate?: boolean;
}) {
  return (
    <div
      className="sdm-progress-track"
      role="progressbar"
      tabIndex={0}
      aria-valuenow={percent}
      aria-valuemin={0}
      aria-valuemax={100}
    >
      <div
        className={
          indeterminate ? "sdm-progress-fill sdm-progress-indeterminate" : "sdm-progress-fill"
        }
        style={indeterminate ? undefined : { width: `${Math.max(0, Math.min(100, percent))}%` }}
      />
    </div>
  );
}
