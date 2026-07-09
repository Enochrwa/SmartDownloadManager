-- Sprint 12: full-text + filtered search across download history and the
-- active queue (docs/SPRINT_PLAN_PHASE2.md Sprint 12), backed by SQLite
-- FTS5 per docs/TECH_DECISIONS.md §13's relational-query rationale.
--
-- `jobs.category` carryover note: docs/SPRINT_PLAN.md's Sprint 5 queue/
-- category system is referenced by later sprints (Sprint 10's playlist
-- grouping, this sprint's search filters) but no migration ever actually
-- added the column — same kind of silent debt Sprint 7 called out and
-- closed for FTP/FTPS carried over from Phase 1. Closing it here: the
-- column is added now so `sdm search --category` and the REST/CLI/UI
-- filters have something real to query against. Existing rows get NULL
-- (uncategorized), same as they are today in every UI that doesn't yet
-- set a category.
ALTER TABLE jobs ADD COLUMN category TEXT;
CREATE INDEX IF NOT EXISTS idx_jobs_category ON jobs(category);

-- A plain (non "content="-linked) FTS5 table: given `jobs.id` is TEXT
-- (not an INTEGER PRIMARY KEY rowid alias), an external-content table
-- would need its FTS rowid kept in lockstep with `jobs`' hidden rowid,
-- which is fragile across INSERT/DELETE churn. Duplicating the indexed
-- text into its own table (kept in sync via triggers below) costs a
-- little extra disk for a lot less trigger complexity, and at
-- queue/history scale (thousands, not millions, of rows) that trade is
-- clearly worth it.
CREATE VIRTUAL TABLE IF NOT EXISTS jobs_fts USING fts5(
    job_id UNINDEXED,
    filename,
    url,
    category,
    status,
    tokenize = 'unicode61'
);

-- Keep jobs_fts in sync with jobs. Delete+reinsert on UPDATE rather than
-- trying to do a partial FTS5 UPDATE — simpler, and job rows are updated
-- far less often than segment/progress rows (which never touch this
-- table), so the extra churn is negligible.
CREATE TRIGGER IF NOT EXISTS jobs_fts_ai AFTER INSERT ON jobs BEGIN
    INSERT INTO jobs_fts(job_id, filename, url, category, status)
    VALUES (new.id, new.destination, new.url, coalesce(new.category, ''), new.status);
END;

CREATE TRIGGER IF NOT EXISTS jobs_fts_ad AFTER DELETE ON jobs BEGIN
    DELETE FROM jobs_fts WHERE job_id = old.id;
END;

CREATE TRIGGER IF NOT EXISTS jobs_fts_au AFTER UPDATE ON jobs BEGIN
    DELETE FROM jobs_fts WHERE job_id = old.id;
    INSERT INTO jobs_fts(job_id, filename, url, category, status)
    VALUES (new.id, new.destination, new.url, coalesce(new.category, ''), new.status);
END;

-- Backfill jobs_fts for any rows that existed before this migration ran
-- (the triggers above only fire for future writes).
INSERT INTO jobs_fts(job_id, filename, url, category, status)
SELECT id, destination, url, coalesce(category, ''), status FROM jobs;
