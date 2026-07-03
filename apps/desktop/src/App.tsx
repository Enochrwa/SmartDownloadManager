import { useEffect, useState } from "react";
import { api } from "./api";
import { AddDownloadDialog } from "./components/AddDownloadDialog";
import { QueueList } from "./components/QueueList";
import { SettingsPanel } from "./components/SettingsPanel";
import { useQueueStore } from "./store";

export function App() {
  const jobs = useQueueStore((s) => s.jobs);
  const speedHistory = useQueueStore((s) => s.speedHistory);
  const theme = useQueueStore((s) => s.theme);
  const setTheme = useQueueStore((s) => s.setTheme);
  const setJobs = useQueueStore((s) => s.setJobs);
  const applyEvent = useQueueStore((s) => s.applyEvent);

  const [addOpen, setAddOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [defaultDir, setDefaultDir] = useState("");

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    api
      .listJobs()
      .then(setJobs)
      .catch(() => {
        /* backend not ready yet (e.g. first render before setup finishes) */
      });
    api
      .defaultDownloadDir()
      .then(setDefaultDir)
      .catch(() => {});
    api
      .getSettings()
      .then((settings) => {
        if (settings.theme === "dark" || settings.theme === "light") {
          setTheme(settings.theme);
        }
      })
      .catch(() => {});

    api.onJobEvent(applyEvent).then((fn) => {
      unlisten = fn;
    });

    return () => unlisten?.();
  }, [applyEvent, setJobs, setTheme]);

  const handleThemeChange = (next: "light" | "dark") => {
    setTheme(next);
    api.setSetting("theme", next).catch(() => {});
  };

  const jobList = Object.values(jobs).sort((a, b) => a.id.localeCompare(b.id));

  return (
    <main className="sdm-app">
      <header className="sdm-header">
        <h1>SmartDownloadManager</h1>
        <div className="sdm-header-actions">
          <button type="button" className="sdm-primary" onClick={() => setAddOpen(true)}>
            Add download
          </button>
          <button type="button" onClick={() => setSettingsOpen(true)} aria-label="Open settings">
            ⚙
          </button>
        </div>
      </header>

      <QueueList jobs={jobList} speedHistory={speedHistory} />

      <AddDownloadDialog open={addOpen} defaultDir={defaultDir} onClose={() => setAddOpen(false)} />
      <SettingsPanel
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        theme={theme}
        onThemeChange={handleThemeChange}
        defaultDir={defaultDir}
      />
    </main>
  );
}
