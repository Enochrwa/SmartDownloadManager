import { useState } from "react";
import { api } from "../api";

interface AddDownloadDialogProps {
  open: boolean;
  defaultDir: string;
  onClose: () => void;
}

export function AddDownloadDialog({ open, defaultDir, onClose }: AddDownloadDialogProps) {
  const [url, setUrl] = useState("");
  const [destination, setDestination] = useState("");
  const [connections, setConnections] = useState("auto");
  const [checksum, setChecksum] = useState("");
  const [onDuplicate, setOnDuplicate] = useState("rename");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!open) return null;

  const reset = () => {
    setUrl("");
    setDestination("");
    setConnections("auto");
    setChecksum("");
    setOnDuplicate("rename");
    setError(null);
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
      });
      reset();
      onClose();
    } catch (err) {
      setError(String(err));
    } finally {
      setSubmitting(false);
    }
  };

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

        <label htmlFor="sdm-url">URL</label>
        <input
          id="sdm-url"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          placeholder="https://example.com/file.zip"
        />

        <label htmlFor="sdm-destination">Save to (optional)</label>
        <input
          id="sdm-destination"
          value={destination}
          onChange={(e) => setDestination(e.target.value)}
          placeholder={defaultDir ? `${defaultDir}/…` : "Default download folder"}
        />

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

        <label htmlFor="sdm-checksum">Expected checksum (optional)</label>
        <input
          id="sdm-checksum"
          value={checksum}
          onChange={(e) => setChecksum(e.target.value)}
          placeholder="sha256:abcd1234…"
        />

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
