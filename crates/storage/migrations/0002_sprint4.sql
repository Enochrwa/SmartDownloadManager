-- Sprint 4 schema additions: checksum verification, per-chunk corruption
-- detection, and mirror support.

ALTER TABLE jobs ADD COLUMN checksum_algorithm TEXT;
ALTER TABLE jobs ADD COLUMN checksum_expected  TEXT;
ALTER TABLE jobs ADD COLUMN checksum_actual    TEXT;
ALTER TABLE jobs ADD COLUMN checksum_verified  INTEGER NOT NULL DEFAULT 0;

-- Per-chunk hashes, computed as each segment lands on disk. Used for
-- targeted corruption detection + repair (only the bad chunk is
-- re-fetched, not the whole file).
CREATE TABLE IF NOT EXISTS chunks (
    id                TEXT PRIMARY KEY,
    job_id            TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    seq               INTEGER NOT NULL,
    start_byte        INTEGER NOT NULL,
    end_byte          INTEGER NOT NULL,
    crc32             INTEGER NOT NULL,
    created_at        TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_chunks_job_id ON chunks(job_id);

-- Mirrors: additional URLs for the same logical download. `seq = 0` is
-- always the primary URL supplied by the caller.
CREATE TABLE IF NOT EXISTS mirrors (
    id                TEXT PRIMARY KEY,
    job_id            TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    url               TEXT NOT NULL,
    seq               INTEGER NOT NULL,
    latency_ms        INTEGER,
    failure_count     INTEGER NOT NULL DEFAULT 0,
    last_used_at      TEXT,
    created_at        TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_mirrors_job_id ON mirrors(job_id);

-- Speeds up duplicate detection lookups by URL/destination filename.
CREATE INDEX IF NOT EXISTS idx_jobs_url ON jobs(url);
CREATE INDEX IF NOT EXISTS idx_jobs_destination ON jobs(destination);
CREATE INDEX IF NOT EXISTS idx_jobs_checksum_actual ON jobs(checksum_actual);
