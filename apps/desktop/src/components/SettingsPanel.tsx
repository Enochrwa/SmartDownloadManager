import type { RepairReport } from "@sdm/common-types";
import { useState } from "react";
import { api } from "../api";
import type { Theme } from "../store";

interface SettingsPanelProps {
  open: boolean;
  onClose: () => void;
  theme: Theme;
  onThemeChange: (theme: Theme) => void;
  defaultDir: string;
}

function describeRepair(report: RepairReport): string {
  switch (report.action) {
    case "none_needed":
      return "Database is healthy — no repair needed.";
    case "restored_from_backup":
      return `Database was corrupt; restored from backup (${report.detail ?? "unknown snapshot"}).`;
    case "recreated_empty":
      return "Database was corrupt and no backup was available; started a fresh database.";
    default:
      return "Unknown repair outcome.";
  }
}

export function SettingsPanel({
  open,
  onClose,
  theme,
  onThemeChange,
  defaultDir,
}: SettingsPanelProps) {
  const [status, setStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (!open) return null;

  const runAction = async (label: string, action: () => Promise<string>) => {
    setBusy(true);
    setStatus(`${label}…`);
    try {
      setStatus(await action());
    } catch (err) {
      setStatus(`${label} failed: ${err}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: backdrop click-to-close is paired with the Close button, which is keyboard-accessible.
    <div className="sdm-modal-backdrop" onClick={onClose}>
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: stopPropagation-only handler, not a standalone interactive action. */}
      <div className="sdm-modal" onClick={(e) => e.stopPropagation()} aria-label="Settings">
        <h2>Settings</h2>

        <section>
          <h3>Appearance</h3>
          <div className="sdm-form-row">
            <label>
              <input
                type="radio"
                name="theme"
                checked={theme === "light"}
                onChange={() => onThemeChange("light")}
              />
              Light
            </label>
            <label>
              <input
                type="radio"
                name="theme"
                checked={theme === "dark"}
                onChange={() => onThemeChange("dark")}
              />
              Dark
            </label>
          </div>
        </section>

        <section>
          <h3>Downloads</h3>
          <p className="sdm-muted">Default folder: {defaultDir}</p>
        </section>

        <section>
          <h3>Recovery</h3>
          <div className="sdm-form-row">
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                runAction("Backing up", async () => {
                  const path = await api.backupNow();
                  return `Backup saved to ${path}`;
                })
              }
            >
              Back up now
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                runAction("Checking database", async () =>
                  describeRepair(await api.repairDatabase()),
                )
              }
            >
              Check &amp; repair database
            </button>
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                runAction("Scanning for orphaned files", async () => {
                  const orphans = await api.cleanupOrphans(true);
                  return orphans.length === 0
                    ? "No orphaned files found."
                    : `Removed ${orphans.length} orphaned file(s).`;
                })
              }
            >
              Clean up orphaned files
            </button>
          </div>
          {status && <p className="sdm-muted">{status}</p>}
        </section>

        <div className="sdm-modal-actions">
          <button type="button" className="sdm-primary" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
