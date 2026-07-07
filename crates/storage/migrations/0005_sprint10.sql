-- Sprint 10: yt-dlp/FFmpeg media extraction.
--
-- `parent_job_id` links a playlist/channel/album's child video Jobs back
-- to the one parent Job the queue/UI groups them under. Sprint 5's
-- dedicated queue/category system (docs/SPRINT_PLAN.md) was never
-- actually implemented in this codebase (crates/scheduler is still a
-- placeholder — see its module doc comment), so rather than depend on
-- nonexistent infrastructure, playlist expansion uses this minimal
-- self-referencing link directly on `jobs`.
ALTER TABLE jobs ADD COLUMN parent_job_id TEXT REFERENCES jobs(id);

CREATE INDEX IF NOT EXISTS idx_jobs_parent_job_id ON jobs(parent_job_id);

-- One row per media (yt-dlp-backed) job: metadata surfaced through the
-- Job model per Sprint 10 scope (title, thumbnail, duration, chapters,
-- available formats, live status), plus which format/subtitle/thumbnail
-- options were actually requested.
CREATE TABLE IF NOT EXISTS media_meta (
    job_id TEXT PRIMARY KEY REFERENCES jobs(id),
    title TEXT,
    thumbnail_url TEXT,
    duration_seconds REAL,
    -- JSON-encoded Vec<ChapterInfo> / Vec<FormatInfo> (crates/media) —
    -- both are extractor-dependent, variable-shape lists that don't
    -- warrant their own normalized tables for what's read back as a
    -- single unit alongside the rest of this row.
    chapters_json TEXT,
    formats_json TEXT,
    is_live INTEGER NOT NULL DEFAULT 0,
    selected_format_id TEXT,
    subtitle_langs_json TEXT,
    embed_thumbnail INTEGER NOT NULL DEFAULT 0
);
