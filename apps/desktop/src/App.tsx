import { useState } from "react";

/**
 * Placeholder shell. Sprint 6 (docs/SPRINT_PLAN.md) replaces this with the
 * real queue view, add-download dialog, and live speed graph.
 */
export function App() {
  const [url, setUrl] = useState("");

  return (
    <main style={{ fontFamily: "system-ui", padding: "2rem" }}>
      <h1>SmartDownloadManager</h1>
      <p>Engine wiring in progress — see docs/SPRINT_PLAN.md.</p>
      <input
        value={url}
        onChange={(e) => setUrl(e.target.value)}
        placeholder="Paste a URL to download"
        style={{ width: "100%", padding: "0.5rem" }}
      />
    </main>
  );
}
