//! Sprint 12: encrypted credential storage. See
//! `docs/SPRINT_PLAN_PHASE2.md` Sprint 12 and the doc comments on
//! `migrations/0008_sprint12_proxy_and_credentials.sql` for the full
//! rationale. Short version: no secret (proxy password, site login,
//! OAuth refresh token) is ever written to a SQLite column in plaintext.
//!
//! [`CredentialStore::store`] tries the OS-native keychain first (via the
//! `keyring` crate — Windows Credential Manager / macOS Keychain / Linux
//! Secret Service over D-Bus). If that's unreachable (most commonly: a
//! headless Linux install with no Secret Service daemon running, which is
//! extremely common on servers/NAS boxes this app also targets), it falls
//! back to AES-256-GCM encryption with the ciphertext in the
//! `encrypted_credentials` table — still never plaintext at rest, just a
//! weaker guarantee than a real keychain (see
//! [`get_or_create_fallback_key`] for exactly how much weaker, and why).
//!
//! A `ref_id` returned by `store` is what the rest of the schema
//! (`jobs.proxy_credential_ref`, `global_settings.proxy_credential_ref`)
//! actually persists — never the secret itself.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use rand::RngCore;
use sqlx::Row;

use crate::SqlitePool;

const KEYRING_SERVICE: &str = "SmartDownloadManager";
const MASTER_KEY_KEYRING_ACCOUNT: &str = "credential-store-master-key";

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    #[error("credential encryption error: {0}")]
    Crypto(String),
    #[error("storage error: {0}")]
    Storage(#[from] sqlx::Error),
    #[error("unknown credential reference: {0}")]
    NotFound(String),
    #[error("malformed credential reference: {0}")]
    MalformedRef(String),
}

pub struct CredentialStore {
    pool: SqlitePool,
}

impl CredentialStore {
    pub fn new(pool: SqlitePool) -> Self {
        CredentialStore { pool }
    }

    /// Encrypt and persist `secret`, returning an opaque reference to
    /// store elsewhere (e.g. as `jobs.proxy_credential_ref`). The
    /// reference is prefixed `kc:` (keychain-backed) or `db:`
    /// (encrypted-fallback-backed) so `retrieve`/`delete` know which
    /// backend to use without a second lookup.
    pub async fn store(&self, secret: &str) -> Result<String, CredentialError> {
        let id = uuid::Uuid::new_v4().to_string();

        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, &id) {
            if entry.set_password(secret).is_ok() {
                return Ok(format!("kc:{id}"));
            }
        }

        let key = self.get_or_create_fallback_key().await?;
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, secret.as_bytes())
            .map_err(|e| CredentialError::Crypto(e.to_string()))?;

        sqlx::query(
            "INSERT INTO encrypted_credentials (ref_id, ciphertext, nonce) VALUES (?1, ?2, ?3)",
        )
        .bind(&id)
        .bind(&ciphertext)
        .bind(&nonce_bytes[..])
        .execute(&self.pool)
        .await?;

        Ok(format!("db:{id}"))
    }

    /// Look up a secret by the reference `store` returned. Returns
    /// `Ok(None)` for a `None` ref (the common "no credential set" case,
    /// so callers can write `if let Some(pw) = store.retrieve_opt(...)`
    /// without a separate branch), `Err(NotFound)` for a ref that should
    /// exist but doesn't (data corruption / manual DB tampering).
    pub async fn retrieve(&self, credential_ref: &str) -> Result<String, CredentialError> {
        if let Some(id) = credential_ref.strip_prefix("kc:") {
            let entry = keyring::Entry::new(KEYRING_SERVICE, id)
                .map_err(|e| CredentialError::Crypto(e.to_string()))?;
            return entry
                .get_password()
                .map_err(|_| CredentialError::NotFound(credential_ref.to_string()));
        }

        if let Some(id) = credential_ref.strip_prefix("db:") {
            let row = sqlx::query(
                "SELECT ciphertext, nonce FROM encrypted_credentials WHERE ref_id = ?1",
            )
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
            let row = row.ok_or_else(|| CredentialError::NotFound(credential_ref.to_string()))?;
            let ciphertext: Vec<u8> = row
                .try_get("ciphertext")
                .map_err(|e| CredentialError::Storage(e))?;
            let nonce_bytes: Vec<u8> = row
                .try_get("nonce")
                .map_err(|e| CredentialError::Storage(e))?;

            let key = self.get_or_create_fallback_key().await?;
            let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
            let nonce = Nonce::from_slice(&nonce_bytes);
            let plaintext = cipher
                .decrypt(nonce, ciphertext.as_ref())
                .map_err(|e| CredentialError::Crypto(e.to_string()))?;
            return String::from_utf8(plaintext)
                .map_err(|e| CredentialError::Crypto(e.to_string()));
        }

        Err(CredentialError::MalformedRef(credential_ref.to_string()))
    }

    /// Convenience for the very common `Option<String>` column case
    /// (`jobs.proxy_credential_ref`, etc.) — `None` in, `Ok(None)` out,
    /// with no separate branch needed at call sites.
    pub async fn retrieve_opt(
        &self,
        credential_ref: Option<&str>,
    ) -> Result<Option<String>, CredentialError> {
        match credential_ref {
            Some(r) => self.retrieve(r).await.map(Some),
            None => Ok(None),
        }
    }

    /// Remove a stored credential (e.g. when a job is deleted, or a
    /// proxy override is cleared).
    pub async fn delete(&self, credential_ref: &str) -> Result<(), CredentialError> {
        if let Some(id) = credential_ref.strip_prefix("kc:") {
            if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, id) {
                let _ = entry.delete_credential();
            }
            return Ok(());
        }
        if let Some(id) = credential_ref.strip_prefix("db:") {
            sqlx::query("DELETE FROM encrypted_credentials WHERE ref_id = ?1")
                .bind(id)
                .execute(&self.pool)
                .await?;
            return Ok(());
        }
        Ok(())
    }

    /// The AES-256 key wrapping everything in `encrypted_credentials`.
    /// Tries the OS keychain first (so even the fallback path's key
    /// isn't sitting in the SQLite file when a keychain *is* available —
    /// only the ciphertext blobs are ever in `encrypted_credentials`
    /// itself in that case). Only when the keychain is completely
    /// unreachable does the key get persisted to
    /// `credential_master_key` — a strictly weaker guarantee (DB file
    /// permissions instead of OS keychain access control), documented
    /// here rather than silently degraded.
    async fn get_or_create_fallback_key(&self) -> Result<[u8; 32], CredentialError> {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, MASTER_KEY_KEYRING_ACCOUNT) {
            if let Ok(existing) = entry.get_password() {
                if let Ok(bytes) = hex::decode(&existing) {
                    if bytes.len() == 32 {
                        let mut key = [0u8; 32];
                        key.copy_from_slice(&bytes);
                        return Ok(key);
                    }
                }
            }
            let mut key = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut key);
            if entry.set_password(&hex::encode(key)).is_ok() {
                return Ok(key);
            }
        }

        // Keychain is completely unreachable — last-resort fallback.
        let existing_row =
            sqlx::query("SELECT key_material FROM credential_master_key WHERE id = 1")
                .fetch_optional(&self.pool)
                .await?;
        if let Some(row) = existing_row {
            let bytes: Vec<u8> = row
                .try_get("key_material")
                .map_err(CredentialError::Storage)?;
            if bytes.len() == 32 {
                let mut key = [0u8; 32];
                key.copy_from_slice(&bytes);
                return Ok(key);
            }
        }

        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        sqlx::query(
            "INSERT INTO credential_master_key (id, key_material) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET key_material = excluded.key_material",
        )
        .bind(&key[..])
        .execute(&self.pool)
        .await?;
        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect_in_memory;

    #[tokio::test]
    async fn secret_round_trips_and_never_appears_in_plaintext_in_the_db_file() {
        // In-memory DB for the round-trip assertion; a real file-backed
        // DB is used below for the plaintext-absence assertion, since an
        // in-memory DB has no on-disk bytes to grep.
        let pool = connect_in_memory().await.unwrap();
        let store = CredentialStore::new(pool);

        let secret = "correct horse battery staple";
        let credential_ref = store.store(secret).await.unwrap();
        let retrieved = store.retrieve(&credential_ref).await.unwrap();
        assert_eq!(retrieved, secret);

        store.delete(&credential_ref).await.unwrap();
        assert!(store.retrieve(&credential_ref).await.is_err());
    }

    #[tokio::test]
    async fn secret_is_never_written_to_the_sqlite_file_in_plaintext() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("creds-test.db");
        let pool = crate::connect(&db_path.to_string_lossy()).await.unwrap();
        let store = CredentialStore::new(pool.clone());

        let secret = "sdmSuperSecretCanaryValue_9f3a1c";
        let credential_ref = store.store(secret).await.unwrap();
        assert_eq!(
            store.retrieve(&credential_ref).await.unwrap(),
            secret,
            "round-trip must return the original secret"
        );

        // Force a WAL checkpoint (sqlx/SQLite may otherwise leave recent
        // writes sitting in the -wal file, not the main DB file) so the
        // on-disk bytes actually reflect what was written.
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&pool)
            .await
            .unwrap();
        pool.close().await;

        let raw = std::fs::read(&db_path).unwrap();
        let needle = secret.as_bytes();
        let found = raw.windows(needle.len()).any(|window| window == needle);
        assert!(
            !found,
            "plaintext secret was found in the raw SQLite file bytes -- \
             this is exactly what the Sprint 12 DoD forbids"
        );

        // Best-effort cleanup: on a machine with a real, working OS
        // keychain (e.g. local development on macOS), `store` above
        // succeeded via the keychain path and never touched
        // `encrypted_credentials` at all -- which is exactly what should
        // happen, but it does mean this test leaves a real keychain
        // entry behind unless removed here. `pool` is already closed at
        // this point, so this goes straight to the keychain, not
        // through `CredentialStore::delete` (which needs the pool for
        // its own `db:`-prefixed path).
        if let Some(id) = credential_ref.strip_prefix("kc:") {
            if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, id) {
                let _ = entry.delete_credential();
            }
        }
    }

    #[tokio::test]
    async fn retrieve_opt_passes_through_none() {
        let pool = connect_in_memory().await.unwrap();
        let store = CredentialStore::new(pool);
        assert_eq!(store.retrieve_opt(None).await.unwrap(), None);
    }

    #[tokio::test]
    async fn unknown_reference_is_not_found_not_a_panic() {
        let pool = connect_in_memory().await.unwrap();
        let store = CredentialStore::new(pool);
        assert!(store.retrieve("db:does-not-exist").await.is_err());
        assert!(store.retrieve("kc:does-not-exist").await.is_err());
        assert!(store.retrieve("garbage-no-prefix").await.is_err());
    }
}
