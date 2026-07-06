-- Sprint 9: Metalink, HLS, MPEG-DASH (docs/SPRINT_PLAN_PHASE2.md).
--
-- Metalink needs no schema change at all: it's resolved into an ordinary
-- HTTP job with mirrors + an expected checksum (job_kind stays 'http'),
-- reusing the Sprint 4 `mirrors` table and `jobs.checksum_*` columns
-- exactly as-is.
--
-- HLS and MPEG-DASH *do* need new job kinds ('hls', 'dash' — no CHECK
-- constraint exists on `jobs.job_kind`, it's a free-form TEXT column per
-- 0003_sprint7.sql, so no ALTER is needed there) plus a place to persist
-- which variant/representations were selected and which segments have
-- already landed on disk, so a resumed job doesn't re-download segments
-- it already has.
CREATE TABLE manifest_meta (
    job_id TEXT PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
    manifest_kind TEXT NOT NULL, -- 'hls' | 'dash'
    manifest_url TEXT NOT NULL,
    -- HLS only: the resolved media playlist URL actually being downloaded
    -- (may equal manifest_url if it wasn't a master playlist).
    media_playlist_url TEXT,
    -- HLS only: human-readable description of the selected variant.
    selected_variant TEXT,
    -- DASH only: which Period/AdaptationSet/Representation were picked
    -- for each track ('video'/'audio'), so a resume doesn't need to
    -- re-run selection against a possibly-changed manifest.
    video_representation_id TEXT,
    audio_representation_id TEXT,
    is_live BOOLEAN NOT NULL DEFAULT 0
);

-- One row per segment (init or media) per track, for both HLS (single
-- 'video' track carrying muxed audio+video, the common case) and DASH
-- (separate 'video'/'audio' tracks). `downloaded` lets a resumed job skip
-- straight to the first not-yet-fetched segment instead of re-probing
-- every temp file on disk.
CREATE TABLE manifest_segments (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    track TEXT NOT NULL, -- 'video' | 'audio' | 'single'
    kind TEXT NOT NULL, -- 'init' | 'media'
    seq INTEGER NOT NULL,
    url TEXT NOT NULL,
    downloaded BOOLEAN NOT NULL DEFAULT 0,
    UNIQUE(job_id, track, kind, seq)
);

CREATE INDEX idx_manifest_segments_job ON manifest_segments(job_id, track, seq);
