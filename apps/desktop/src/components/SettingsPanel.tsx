import type { PairingStatus, PairingToken, RepairReport } from "@sdm/common-types";
import { useEffect, useState } from "react";
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
  const [pairing, setPairing] = useState<PairingStatus | null>(null);
  const [issuedToken, setIssuedToken] = useState<PairingToken | null>(null);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    const poll = async () => {
      try {
        const result = await api.pairingStatus();
        if (!cancelled) setPairing(result);
      } catch {
        // sdmd's embedded extension API may not have finished starting
        // up yet on the very first open — the next poll will pick it up.
      }
    };
    void poll();
    const interval = setInterval(poll, 3000);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [open]);

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

  const handleGenerateToken = async () => {
    setBusy(true);
    try {
      setIssuedToken(await api.pairingIssueToken());
    } catch (err) {
      setStatus(`Couldn't generate a pairing token: ${err}`);
    } finally {
      setBusy(false);
    }
  };

  const handleRevoke = async (token: string) => {
    setBusy(true);
    try {
      await api.pairingRevokeToken(token);
      setPairing(await api.pairingStatus());
    } catch (err) {
      setStatus(`Couldn't revoke that pairing: ${err}`);
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
          <h3>Browser Extension</h3>
          <p className="sdm-form-row">
            <span
              className={
                pairing?.connected
                  ? "sdm-status-dot sdm-connected"
                  : "sdm-status-dot sdm-disconnected"
              }
              aria-hidden="true"
            />
            {pairing?.connected
              ? "Extension connected"
              : pairing?.apiPort
                ? "No extension connected"
                : pairing?.apiError
                  ? `Extension API unavailable: ${pairing.apiError}`
                  : "Checking…"}
          </p>
          {pairing && !pairing.apiPort && pairing.apiError && (
            <p className="sdm-muted sdm-error">
              The embedded extension API couldn't start, so pairing won't work until this is
              resolved. This usually means another SmartDownloadManager instance (or something else)
              is already using the port — quitting other instances and reopening Settings will retry
              automatically.
            </p>
          )}
          {pairing && pairing.pairedExtensions.length > 0 && (
            <ul className="sdm-muted">
              {pairing.pairedExtensions.map((ext) => (
                <li key={ext.createdAt + ext.label}>
                  {ext.label}
                  {ext.lastSeenAt
                    ? ` — last seen ${new Date(ext.lastSeenAt).toLocaleString()}`
                    : ""}
                </li>
              ))}
            </ul>
          )}
          <div className="sdm-form-row">
            <button
              type="button"
              disabled={busy || !pairing?.apiPort}
              onClick={() => void handleGenerateToken()}
              title={
                !pairing?.apiPort
                  ? "Waiting for the extension API to start before a token can be issued"
                  : undefined
              }
            >
              Generate pairing token
            </button>
          </div>
          {issuedToken && pairing?.apiPort && (
            <div className="sdm-pairing-token">
              <p className="sdm-muted">
                In the extension's Settings page, set the sdmd address to{" "}
                <code>http://127.0.0.1:{pairing.apiPort}</code> and paste this token:
              </p>
              <code className="sdm-token">{issuedToken.token}</code>
              <div className="sdm-form-row">
                <button
                  type="button"
                  onClick={() => void navigator.clipboard.writeText(issuedToken.token)}
                >
                  Copy token
                </button>
                <button type="button" onClick={() => handleRevoke(issuedToken.token)}>
                  Revoke this token
                </button>
              </div>
            </div>
          )}
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
