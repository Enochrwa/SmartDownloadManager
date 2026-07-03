// Formatting helpers shared by every SmartDownloadManager client surface.

export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || Number.isNaN(bytes)) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes;
  let unitIndex = -1;
  do {
    value /= 1024;
    unitIndex += 1;
  } while (value >= 1024 && unitIndex < units.length - 1);
  return `${value.toFixed(value < 10 ? 2 : 1)} ${units[unitIndex]}`;
}

export function formatSpeed(bytesPerSecond: number | null | undefined): string {
  if (!bytesPerSecond || bytesPerSecond <= 0) return "0 B/s";
  return `${formatBytes(bytesPerSecond)}/s`;
}

export function formatPercent(downloaded: number, total?: number | null): number {
  if (!total || total <= 0) return 0;
  return Math.min(100, Math.round((downloaded / total) * 100));
}

export function formatEta(
  downloaded: number,
  total: number | null | undefined,
  bps: number,
): string {
  if (!total || total <= downloaded || bps <= 0) return "—";
  const remainingSeconds = (total - downloaded) / bps;
  if (remainingSeconds < 60) return `${Math.ceil(remainingSeconds)}s`;
  if (remainingSeconds < 3600) return `${Math.ceil(remainingSeconds / 60)}m`;
  return `${(remainingSeconds / 3600).toFixed(1)}h`;
}
