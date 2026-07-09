-- Sprint 12: proxy configuration persistence + the encrypted-credential
-- indirection layer (docs/SPRINT_PLAN_PHASE2.md Sprint 12).
--
-- Secrets (proxy passwords, site logins, OAuth refresh tokens) are never
-- stored in this table or any other SQLite column. `credential_ref` is an
-- opaque lookup key into the OS-native keychain (Windows Credential
-- Manager / macOS Keychain / Linux Secret Service, via the `keyring`
-- crate -- see sdm_storage::credentials) with an AES-256-GCM-encrypted
-- fallback for headless Linux installs with no Secret Service daemon
-- running (docs/TECH_DECISIONS.md already commits to SQLite everywhere
-- else, so the fallback ciphertext lives in `encrypted_credentials`
-- below rather than a second storage engine -- but it's still never a
-- plaintext secret at rest either way).

-- A single global settings row (id is always 1) rather than a generic
-- key-value table: Sprint 12 only needs one setting so far (the global
-- default proxy), and a typed table catches a typo'd key at compile time
-- instead of silently no-op'ing a `WHERE key = 'proxie'`. If a second
-- unrelated global setting shows up in a later sprint, that's the moment
-- to revisit this as key-value instead of guessing now.
CREATE TABLE IF NOT EXISTS global_settings (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    proxy_url TEXT,
    -- Opaque reference into the keychain (or encrypted_credentials
    -- fallback below) for the proxy username/password pair, if any.
    -- NULL means either no proxy or an unauthenticated proxy.
    proxy_credential_ref TEXT,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Sprint 12 DoD calls for per-Job proxy override too. Stored as a
-- credential_ref (for the auth half) plus the (non-secret) proxy URL
-- directly on the job row -- symmetric with global_settings above.
ALTER TABLE jobs ADD COLUMN proxy_url TEXT;
ALTER TABLE jobs ADD COLUMN proxy_credential_ref TEXT;

-- The AES-256-GCM-encrypted fallback store for platforms with no
-- reachable OS keychain (see sdm_storage::credentials::CredentialStore).
-- `ref_id` is what `proxy_credential_ref` / a future site-login row
-- points at. `nonce` is the 12-byte GCM nonce, freshly random per
-- encryption (AES-GCM must never reuse a nonce under the same key).
-- The encryption key itself is *not* in this table -- it's generated
-- once and stored in the OS keychain under a fixed service/account
-- whenever a keychain is reachable, so even this fallback path keeps
-- the actual key out of the SQLite file in the common case (only the
-- ciphertext blobs live here then). Only when the keychain is
-- completely unreachable does the key itself fall back to
-- `credential_master_key` below -- see that table's comment for exactly
-- how much weaker that degraded mode is, and why it's still an
-- improvement over plaintext.
CREATE TABLE IF NOT EXISTS encrypted_credentials (
    ref_id TEXT PRIMARY KEY,
    ciphertext BLOB NOT NULL,
    nonce BLOB NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Wraps the AES key used for `encrypted_credentials` above. Only ever
-- populated as a *last-resort* fallback when the OS keychain itself is
-- unreachable for storing that wrapping key (e.g. a headless Linux box
-- with no Secret Service daemon and no keychain at all) -- see
-- sdm_storage::credentials::get_or_create_fallback_key. This is a weaker
-- guarantee than the keychain path (the key sits in the same SQLite file
-- as the ciphertext it protects, so DB file permissions are now doing
-- the work a keychain normally would), but it's still strictly better
-- than plaintext secrets, and it's the documented, honest degraded mode
-- rather than a silent one.
CREATE TABLE IF NOT EXISTS credential_master_key (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    key_material BLOB NOT NULL
);
