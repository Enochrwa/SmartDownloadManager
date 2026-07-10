//! Sprint 12: authentication config storage — cookie-based sessions,
//! bearer token/API key headers (per-Job or per-domain), and OAuth2
//! tokens. See `migrations/0009_sprint12_auth.sql` for the schema
//! rationale. Mirrors the `ProxySettings`/`get_global_proxy`/
//! `set_job_proxy` pattern in `crate::lib` — the one difference is that
//! the secret-bearing payload here (headers + cookie, or OAuth tokens) is
//! a small JSON blob rather than a single string, so it's JSON-encoded
//! before being handed to `CredentialStore::store` rather than stored as
//! a raw string.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::credentials::{CredentialError, CredentialStore};
use crate::SqlitePool;

/// Which auth_configs row a lookup/write targets. No `Global` variant —
/// see the migration's doc comment for why a blanket "send this header to
/// every site" scope is deliberately not offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthScope {
    Domain(String),
    Job(String),
}

impl AuthScope {
    fn column_value(&self) -> (&'static str, &str) {
        match self {
            AuthScope::Domain(d) => ("domain", d.as_str()),
            AuthScope::Job(j) => ("job", j.as_str()),
        }
    }
}

/// The decrypted, in-process view of one auth_configs row — a set of
/// custom headers (bearer tokens, API keys, or anything else a site's
/// login flow needs) plus an optional raw `Cookie` header value.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthConfig {
    /// `(name, value)` pairs, applied as request headers.
    pub headers: Vec<(String, String)>,
    /// Raw `Cookie:` header value, e.g. `"sessionid=abc; csrftoken=xyz"`.
    pub cookie: Option<String>,
}

impl AuthConfig {
    pub fn is_empty(&self) -> bool {
        self.headers.is_empty() && self.cookie.is_none()
    }
}

/// Persist (or replace) the auth config for `scope`, encrypting the
/// header/cookie payload via `store` before it ever reaches SQLite —
/// same never-plaintext-at-rest guarantee as `CredentialStore` gives
/// proxy passwords.
pub async fn set_auth_config(
    pool: &SqlitePool,
    store: &CredentialStore,
    scope: &AuthScope,
    config: &AuthConfig,
) -> Result<(), CredentialError> {
    // Clearing an existing config's old credential first avoids leaking
    // an orphaned keychain/encrypted-fallback entry every time a config
    // is updated in place.
    delete_auth_config(pool, store, scope).await?;

    if config.is_empty() {
        return Ok(());
    }

    let payload = serde_json::to_string(config)
        .map_err(|e| CredentialError::Crypto(format!("auth config serialization: {e}")))?;
    let credential_ref = store.store(&payload).await?;

    let (scope_name, scope_key) = scope.column_value();
    let id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO auth_configs (id, scope, scope_key, credential_ref, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(scope, scope_key) DO UPDATE SET
            credential_ref = excluded.credential_ref,
            updated_at = excluded.updated_at",
    )
    .bind(&id)
    .bind(scope_name)
    .bind(scope_key)
    .bind(&credential_ref)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(CredentialError::Storage)?;
    Ok(())
}

pub async fn get_auth_config(
    pool: &SqlitePool,
    store: &CredentialStore,
    scope: &AuthScope,
) -> Result<Option<AuthConfig>, CredentialError> {
    let (scope_name, scope_key) = scope.column_value();
    let row =
        sqlx::query("SELECT credential_ref FROM auth_configs WHERE scope = ?1 AND scope_key = ?2")
            .bind(scope_name)
            .bind(scope_key)
            .fetch_optional(pool)
            .await
            .map_err(CredentialError::Storage)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let credential_ref: String = row
        .try_get("credential_ref")
        .map_err(CredentialError::Storage)?;
    let payload = store.retrieve(&credential_ref).await?;
    let config: AuthConfig = serde_json::from_str(&payload)
        .map_err(|e| CredentialError::Crypto(format!("auth config deserialization: {e}")))?;
    Ok(Some(config))
}

pub async fn delete_auth_config(
    pool: &SqlitePool,
    store: &CredentialStore,
    scope: &AuthScope,
) -> Result<(), CredentialError> {
    let (scope_name, scope_key) = scope.column_value();
    let row =
        sqlx::query("SELECT credential_ref FROM auth_configs WHERE scope = ?1 AND scope_key = ?2")
            .bind(scope_name)
            .bind(scope_key)
            .fetch_optional(pool)
            .await
            .map_err(CredentialError::Storage)?;
    if let Some(row) = row {
        let credential_ref: String = row
            .try_get("credential_ref")
            .map_err(CredentialError::Storage)?;
        store.delete(&credential_ref).await?;
    }
    sqlx::query("DELETE FROM auth_configs WHERE scope = ?1 AND scope_key = ?2")
        .bind(scope_name)
        .bind(scope_key)
        .execute(pool)
        .await
        .map_err(CredentialError::Storage)?;
    Ok(())
}

/// Resolution order: a Job-scoped override, if `job_id` is given and one
/// exists, wins outright; otherwise fall back to whatever domain the
/// `url`'s host matches. Returns `None` (not an error) when neither
/// applies — "no auth configured" is the overwhelmingly common case.
pub async fn resolve_auth_config(
    pool: &SqlitePool,
    store: &CredentialStore,
    job_id: Option<&str>,
    url: &str,
) -> Result<Option<AuthConfig>, CredentialError> {
    if let Some(job_id) = job_id {
        if let Some(cfg) = get_auth_config(pool, store, &AuthScope::Job(job_id.to_string())).await?
        {
            return Ok(Some(cfg));
        }
    }
    let Some(host) = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
    else {
        return Ok(None);
    };
    get_auth_config(pool, store, &AuthScope::Domain(host)).await
}

/// One row's decrypted OAuth2 token set for a domain.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: String,
    /// RFC3339 expiry, if the provider gave one.
    pub expires_at: Option<String>,
}

pub async fn store_oauth_tokens(
    pool: &SqlitePool,
    store: &CredentialStore,
    domain: &str,
    tokens: &OAuthTokens,
) -> Result<(), CredentialError> {
    delete_oauth_tokens(pool, store, domain).await?;
    let payload = serde_json::to_string(tokens)
        .map_err(|e| CredentialError::Crypto(format!("oauth token serialization: {e}")))?;
    let credential_ref = store.store(&payload).await?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO oauth_tokens (id, domain, credential_ref, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(domain) DO UPDATE SET
            credential_ref = excluded.credential_ref,
            updated_at = excluded.updated_at",
    )
    .bind(&id)
    .bind(domain)
    .bind(&credential_ref)
    .bind(&now)
    .execute(pool)
    .await
    .map_err(CredentialError::Storage)?;
    Ok(())
}

pub async fn get_oauth_tokens(
    pool: &SqlitePool,
    store: &CredentialStore,
    domain: &str,
) -> Result<Option<OAuthTokens>, CredentialError> {
    let row = sqlx::query("SELECT credential_ref FROM oauth_tokens WHERE domain = ?1")
        .bind(domain)
        .fetch_optional(pool)
        .await
        .map_err(CredentialError::Storage)?;
    let Some(row) = row else {
        return Ok(None);
    };
    let credential_ref: String = row
        .try_get("credential_ref")
        .map_err(CredentialError::Storage)?;
    let payload = store.retrieve(&credential_ref).await?;
    let tokens: OAuthTokens = serde_json::from_str(&payload)
        .map_err(|e| CredentialError::Crypto(format!("oauth token deserialization: {e}")))?;
    Ok(Some(tokens))
}

pub async fn delete_oauth_tokens(
    pool: &SqlitePool,
    store: &CredentialStore,
    domain: &str,
) -> Result<(), CredentialError> {
    let row = sqlx::query("SELECT credential_ref FROM oauth_tokens WHERE domain = ?1")
        .bind(domain)
        .fetch_optional(pool)
        .await
        .map_err(CredentialError::Storage)?;
    if let Some(row) = row {
        let credential_ref: String = row
            .try_get("credential_ref")
            .map_err(CredentialError::Storage)?;
        store.delete(&credential_ref).await?;
    }
    sqlx::query("DELETE FROM oauth_tokens WHERE domain = ?1")
        .bind(domain)
        .execute(pool)
        .await
        .map_err(CredentialError::Storage)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect_in_memory;

    #[tokio::test]
    async fn domain_scoped_auth_config_round_trips_and_clears() {
        let pool = connect_in_memory().await.unwrap();
        let store = CredentialStore::new(pool.clone());
        let scope = AuthScope::Domain("example.com".to_string());

        assert_eq!(get_auth_config(&pool, &store, &scope).await.unwrap(), None);

        let cfg = AuthConfig {
            headers: vec![("Authorization".to_string(), "Bearer tok".to_string())],
            cookie: Some("session=abc".to_string()),
        };
        set_auth_config(&pool, &store, &scope, &cfg).await.unwrap();
        assert_eq!(
            get_auth_config(&pool, &store, &scope).await.unwrap(),
            Some(cfg)
        );

        delete_auth_config(&pool, &store, &scope).await.unwrap();
        assert_eq!(get_auth_config(&pool, &store, &scope).await.unwrap(), None);
    }

    #[tokio::test]
    async fn job_scope_overrides_domain_scope_on_resolve() {
        let pool = connect_in_memory().await.unwrap();
        let store = CredentialStore::new(pool.clone());

        let domain_cfg = AuthConfig {
            headers: vec![("X-From".to_string(), "domain".to_string())],
            cookie: None,
        };
        set_auth_config(
            &pool,
            &store,
            &AuthScope::Domain("example.com".to_string()),
            &domain_cfg,
        )
        .await
        .unwrap();

        // No job override yet: resolves to the domain config.
        let resolved =
            resolve_auth_config(&pool, &store, Some("job-1"), "https://example.com/file")
                .await
                .unwrap();
        assert_eq!(resolved, Some(domain_cfg.clone()));

        let job_cfg = AuthConfig {
            headers: vec![("X-From".to_string(), "job".to_string())],
            cookie: None,
        };
        set_auth_config(
            &pool,
            &store,
            &AuthScope::Job("job-1".to_string()),
            &job_cfg,
        )
        .await
        .unwrap();

        let resolved =
            resolve_auth_config(&pool, &store, Some("job-1"), "https://example.com/file")
                .await
                .unwrap();
        assert_eq!(resolved, Some(job_cfg));

        // A different job with no override still falls back to the
        // domain config.
        let resolved =
            resolve_auth_config(&pool, &store, Some("job-2"), "https://example.com/file")
                .await
                .unwrap();
        assert_eq!(resolved, Some(domain_cfg));
    }

    #[tokio::test]
    async fn resolve_returns_none_when_nothing_configured() {
        let pool = connect_in_memory().await.unwrap();
        let store = CredentialStore::new(pool.clone());
        let resolved =
            resolve_auth_config(&pool, &store, Some("job-x"), "https://nowhere.example/")
                .await
                .unwrap();
        assert_eq!(resolved, None);
    }

    #[tokio::test]
    async fn oauth_tokens_round_trip_and_clear_and_no_secret_in_raw_db() {
        let pool = connect_in_memory().await.unwrap();
        let store = CredentialStore::new(pool.clone());

        assert_eq!(
            get_oauth_tokens(&pool, &store, "example.com")
                .await
                .unwrap(),
            None
        );

        let tokens = OAuthTokens {
            access_token: "super-secret-access-token".to_string(),
            refresh_token: Some("super-secret-refresh-token".to_string()),
            token_type: "Bearer".to_string(),
            expires_at: Some("2026-12-31T00:00:00Z".to_string()),
        };
        store_oauth_tokens(&pool, &store, "example.com", &tokens)
            .await
            .unwrap();
        assert_eq!(
            get_oauth_tokens(&pool, &store, "example.com")
                .await
                .unwrap(),
            Some(tokens.clone())
        );

        // Sprint 12 DoD (adapted from the proxy-credential case): the
        // access/refresh token strings must never appear anywhere in the
        // raw auth_configs/oauth_tokens/encrypted_credentials tables —
        // only inside the (encrypted) ciphertext blob.
        let rows = sqlx::query("SELECT ref_id, ciphertext FROM encrypted_credentials")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(
            !rows.is_empty(),
            "expected a fallback-encrypted credential row"
        );
        for row in rows {
            let ciphertext: Vec<u8> = row.try_get("ciphertext").unwrap();
            let as_text = String::from_utf8_lossy(&ciphertext);
            assert!(!as_text.contains("super-secret-access-token"));
            assert!(!as_text.contains("super-secret-refresh-token"));
        }
        let oauth_rows = sqlx::query("SELECT domain, credential_ref FROM oauth_tokens")
            .fetch_all(&pool)
            .await
            .unwrap();
        for row in oauth_rows {
            let credential_ref: String = row.try_get("credential_ref").unwrap();
            assert!(!credential_ref.contains("super-secret"));
        }

        delete_oauth_tokens(&pool, &store, "example.com")
            .await
            .unwrap();
        assert_eq!(
            get_oauth_tokens(&pool, &store, "example.com")
                .await
                .unwrap(),
            None
        );
    }
}
