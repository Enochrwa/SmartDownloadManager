-- Sprint 7: multi-protocol job kinds (BitTorrent via magnet/.torrent, FTP/FTPS).
--
-- `jobs.job_kind` discriminates how `crates/engine` should drive a job:
--   'http'    - existing Sprint 1-6 segmented/single-stream HTTP(S) engine
--   'ftp'     - single-stream FTP/FTPS via sdm-protocols::ftp (this sprint)
--   'torrent' - magnet/.torrent via sdm-torrent (this sprint)
-- Existing rows all default to 'http', which is exactly what they already are.
ALTER TABLE jobs ADD COLUMN job_kind TEXT NOT NULL DEFAULT 'http';

-- Torrent-specific metadata, one row per job with job_kind = 'torrent'.
-- Kept in its own table (mirrors how 0002_sprint4.sql added `mirrors` and
-- `duplicate_of` alongside `jobs` rather than widening the base table with
-- protocol-specific columns that are NULL for every other job kind).
CREATE TABLE torrent_meta (
    job_id TEXT PRIMARY KEY REFERENCES jobs(id) ON DELETE CASCADE,
    info_hash TEXT NOT NULL,
    magnet_uri TEXT,
    torrent_file_path TEXT,
    display_name TEXT,
    piece_count INTEGER,
    file_count INTEGER,
    peer_count INTEGER NOT NULL DEFAULT 0,
    sequential BOOLEAN NOT NULL DEFAULT 0,
    only_files TEXT -- JSON array of selected file indices, NULL = all files
);

CREATE INDEX idx_torrent_meta_info_hash ON torrent_meta(info_hash);
