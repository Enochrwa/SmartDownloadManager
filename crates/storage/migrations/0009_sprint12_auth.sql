-- Sprint 12: authentication config persistence (docs/SPRINT_PLAN_PHASE2.md
-- Sprint 12, "Authentication" scope item) — cookie-based sessions, bearer
-- token/API key headers, and OAuth2 refresh tokens.
--
-- Same non-negotiable as the proxy/credentials migration before this one
-- (0008_sprint12_proxy_and_credentials.sql): no secret (a cookie value, a
-- bearer token, an OAuth refresh token) is ever written to a SQLite column
-- in plaintext. Every secret-bearing row here stores a `credential_ref`
-- (see sdm_storage::credentials::CredentialStore) pointing at the OS
-- keychain / AES-256-GCM-encrypted fallback store, exactly like
-- `jobs.proxy_credential_ref` already does.

-- One row per "scope" an auth config applies to: a specific Job (takes
-- precedence — a job can override whatever its domain would otherwise
-- get), or a domain (applies to every job whose URL host matches, unless
-- that job has its own override). There's no third "global" scope here on
-- purpose: unlike the proxy setting, blanket global auth headers/cookies
-- sent to every site regardless of destination is a credential-leak
-- footgun, not a convenience — Sprint 12's scope text itself only calls
-- for "per-Job or per-domain" headers, not a global default.
--
-- `credential_ref` points at an encrypted JSON blob:
-- `{"headers": [["Name", "value"], ...], "cookie": "raw Cookie header or null"}`
-- — bundled into one blob (rather than one row per header) because a
-- site's auth config is always read and applied as a whole; there's no
-- use case for fetching a single header independent of the rest.
CREATE TABLE IF NOT EXISTS auth_configs (
    id TEXT PRIMARY KEY,
    scope TEXT NOT NULL CHECK (scope IN ('domain', 'job')),
    -- Domain (e.g. "example.com") when scope='domain'; job id when
    -- scope='job'. Never NULL -- unlike global_settings' single-row
    -- table, every auth_configs row is scoped to something specific.
    scope_key TEXT NOT NULL,
    credential_ref TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (scope, scope_key)
);

-- OAuth2 authorization-code-flow tokens, one row per domain (an OAuth
-- client registration is inherently domain/provider-specific -- unlike
-- cookies/headers there's no per-Job OAuth override case in the Sprint 12
-- scope text). `credential_ref` points at an encrypted JSON blob:
-- `{"access_token": "...", "refresh_token": "...", "token_type": "Bearer",
--   "expires_at": "2026-07-09T12:00:00Z" | null}`.
CREATE TABLE IF NOT EXISTS oauth_tokens (
    id TEXT PRIMARY KEY,
    domain TEXT NOT NULL UNIQUE,
    credential_ref TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);
