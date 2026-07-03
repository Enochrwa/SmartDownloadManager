import type { SpeedSample } from "@sdm/common-types";
import { formatSpeed } from "@sdm/ui";

const WIDTH = 160;
const HEIGHT = 36;

export function SpeedGraph({ samples }: { samples: SpeedSample[] }) {
  if (samples.length < 2) {
    return <div className="sdm-speed-graph sdm-speed-graph-empty">—</div>;
  }

  const max = Math.max(...samples.map((s) => s.bps), 1);
  const step = WIDTH / (samples.length - 1);
  const points = samples
    .map((s, i) => {
      const x = i * step;
      const y = HEIGHT - (s.bps / max) * HEIGHT;
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");

  const latest = samples[samples.length - 1]?.bps ?? 0;

  return (
    <div className="sdm-speed-graph" title={`${formatSpeed(latest)} (current)`}>
      <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} width={WIDTH} height={HEIGHT} aria-hidden="true">
        <polyline points={points} fill="none" stroke="var(--sdm-accent)" strokeWidth={1.5} />
      </svg>
      <span className="sdm-speed-graph-label">{formatSpeed(latest)}</span>
    </div>
  );
}
