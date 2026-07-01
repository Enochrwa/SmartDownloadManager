-- Sprint 1 schema: jobs, segments, settings.
-- Segment-level columns (connections, retry tracking, validators) are
-- included from the start so Sprint 2/3 code doesn't need a destructive
-- SQLite column-rename migration later.

CREATE TABLE IF NOT EXISTS jobs (
    id                TEXT PRIMARY KEY,
    url               TEXT NOT NULL,
    destination       TEXT NOT NULL,
    status            TEXT NOT NULL,
    total_bytes       INTEGER,
    downloaded_bytes  INTEGER NOT NULL DEFAULT 0,
    connections       INTEGER NOT NULL DEFAULT 1,
    supports_range    INTEGER NOT NULL DEFAULT 0,
    etag              TEXT,
    last_modified     TEXT,
    error_class       TEXT,
    error_message     TEXT,
    created_at        TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at        TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS segments (
    id                 TEXT PRIMARY KEY,
    job_id             TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    seq                INTEGER NOT NULL,
    start_byte         INTEGER NOT NULL,
    end_byte           INTEGER NOT NULL,
    downloaded_bytes   INTEGER NOT NULL DEFAULT 0,
    status             TEXT NOT NULL,
    retry_count        INTEGER NOT NULL DEFAULT 0,
    last_error_class   TEXT,
    updated_at         TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_segments_job_id ON segments(job_id);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
