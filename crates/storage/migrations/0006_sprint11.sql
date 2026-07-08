-- Sprint 11: browser extension pairing.
--
-- The extension never talks to the engine or filesystem directly (per
-- docs/SPRINT_PLAN_PHASE2.md Sprint 11 and docs/TECH_DECISIONS.md §6) — it
-- authenticates to `sdmd`'s REST/WebSocket API with a bearer token minted
-- during a first-run pairing flow the desktop app / `sdmd` displays to the
-- user. `last_seen_at` is what the desktop app's "Extension connected"
-- status indicator polls (a token seen within the last N seconds counts as
-- connected); `revoked_at` supports revoking a pairing without deleting
-- history of it having existed.
CREATE TABLE IF NOT EXISTS pairing_tokens (
    token TEXT PRIMARY KEY,
    label TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_seen_at TEXT,
    revoked_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_pairing_tokens_last_seen ON pairing_tokens(last_seen_at);
