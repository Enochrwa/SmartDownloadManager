//! `sdm` — the SmartDownloadManager CLI.
//!
//! Drives sdm-engine in-process for one-shot commands (`sdm download <url>`
//! works with no daemon running).
//!
//! Sprint 1: `sdm download <url> [-o output]`.
//! Sprint 2: `--connections`/`-c` (number, or `auto`).
//! Sprint 3: `sdm resume <job-id>`, `sdm list`, `sdm show <job-id>`.
//! Sprint 4: `--mirror` (repeatable), `--checksum algo:hex`,
//! `--on-duplicate overwrite|rename|skip`, `sdm verify <job-id>`.
//! Sprint 8: `sftp://`/`scp://` (`--ssh-key`/`--ssh-password`,
//! `--accept-new-hostkey`), `webdav://`/`webdavs://`, and
//! `sdm list-remote <url>` for FTP/SFTP/WebDAV directory listing.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use sdm_engine::{
    ConnectionsOption, DashDownloadRequest, DownloadRequest, DuplicatePolicy, Engine,
    ExpectedChecksum, FtpDownloadRequest, HlsDownloadRequest, ProgressEvent, ScpDownloadRequest,
    SftpDownloadRequest, SshConnectionOptions, SshEngine, TorrentDownloadRequest,
    WebDavDownloadRequest, WebDavEngine,
};
use sdm_protocols::ssh::{HostKeyPolicy, SshAuth, SshUrl};

#[derive(Parser)]
#[command(name = "sdm", about = "SmartDownloadManager CLI", version)]
struct Cli {
    /// Path to the SQLite database (defaults to $SDM_HOME/jobs.db, or ~/.sdm/jobs.db)
    #[arg(long, global = true)]
    db: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Download a single URL (runs the engine in-process — no daemon needed).
    Download {
        url: String,
        /// Destination file path. Defaults to the last path segment of the URL.
        #[arg(short, long)]
        output: Option<String>,
        /// Number of connections (1-128), or "auto" to size based on file length.
        #[arg(short, long, default_value = "auto")]
        connections: String,
        /// Additional mirror URL serving the same content. Repeatable.
        #[arg(long = "mirror")]
        mirrors: Vec<String>,
        /// Expected checksum as "algorithm:hex", e.g. "sha256:abcd...".
        /// One of md5, sha1, sha256, sha512, crc32. Verified once the
        /// download finishes; a mismatch fails the job.
        #[arg(long)]
        checksum: Option<String>,
        /// What to do if this looks like a duplicate of an existing job
        /// (same URL, same destination filename, or same checksum).
        #[arg(long = "on-duplicate", default_value = "rename")]
        on_duplicate: String,
        /// Torrent only: request best-effort in-order piece priority for
        /// the first file (see docs/SPRINT_PLAN_PHASE2.md Sprint 7).
        #[arg(long)]
        sequential: bool,
        /// Torrent only: comma-separated file indices to download (all
        /// files if omitted). Run `sdm show <job-id>` after adding to see
        /// the file list if you don't already know the indices.
        #[arg(long = "only-files")]
        only_files: Option<String>,
        /// SFTP/SCP only: password (prefer `--ssh-key`; falls back to any
        /// `user:pass@` in the URL itself if omitted).
        #[arg(long = "ssh-password")]
        ssh_password: Option<String>,
        /// SFTP/SCP only: path to a private key file.
        #[arg(long = "ssh-key")]
        ssh_key: Option<String>,
        /// SFTP/SCP only: passphrase for --ssh-key, if it's encrypted.
        #[arg(long = "ssh-key-passphrase")]
        ssh_key_passphrase: Option<String>,
        /// SFTP/SCP only: path to the known_hosts file (defaults to
        /// ~/.ssh/known_hosts).
        #[arg(long = "known-hosts")]
        known_hosts: Option<String>,
        /// SFTP/SCP only: trust and remember a host key seen for the
        /// first time, instead of rejecting it. A host key that
        /// *conflicts* with one already on record is always rejected
        /// regardless of this flag.
        #[arg(long = "accept-new-hostkey")]
        accept_new_hostkey: bool,
        /// HLS only: which variant to download — "best" (default),
        /// "worst", or a 0-based variant index (0 = highest bandwidth).
        #[arg(long, default_value = "best")]
        quality: String,
        /// Force this URL through the yt-dlp-backed extractor (Sprint 10)
        /// even if it isn't one of the handful of sites
        /// `sdm` recognizes automatically — yt-dlp itself supports
        /// thousands of sites we can't enumerate here, so this is the
        /// reliable way to reach any of them.
        #[arg(long = "via-ytdlp")]
        via_ytdlp: bool,
        /// yt-dlp only: which format to fetch — "best" (default, highest
        /// resolution) or an exact format_id from `sdm probe <url>`.
        #[arg(long = "media-quality", default_value = "best")]
        media_quality: String,
        /// yt-dlp only: comma-separated subtitle language codes to fetch
        /// and embed, e.g. "en,fr".
        #[arg(long = "subs")]
        subs: Option<String>,
        /// yt-dlp only: embed the site's thumbnail as cover art.
        #[arg(long = "embed-thumbnail")]
        embed_thumbnail: bool,
        /// Sprint 12: route this download through a proxy —
        /// `socks5h://host:port` (SOCKS5, proxy-side DNS — the usual
        /// choice), `socks4://host:port`, or `http(s)://host:port`.
        /// Overrides the global default proxy (`sdm set-proxy`) for this
        /// download only.
        #[arg(long)]
        proxy: Option<String>,
        /// Username for --proxy, if it requires auth.
        #[arg(long = "proxy-user")]
        proxy_user: Option<String>,
        /// Password for --proxy. Stored encrypted (OS keychain, or an
        /// AES-256-GCM-encrypted fallback if no keychain is reachable —
        /// see `sdm_storage::credentials`) if this job is later resumed;
        /// never written anywhere in plaintext.
        #[arg(long = "proxy-pass")]
        proxy_pass: Option<String>,
        /// Sprint 12: a custom request header, "Name: Value". Repeatable.
        /// Used for API-key auth (e.g. `--header "X-API-Key: abc123"`) or
        /// anything else a site's auth needs beyond a bearer token/cookie.
        #[arg(long = "header")]
        headers: Vec<String>,
        /// Sprint 12: shorthand for `--header "Authorization: Bearer <token>"`.
        #[arg(long)]
        bearer: Option<String>,
        /// Sprint 12: raw `Cookie:` header value for session-cookie auth,
        /// e.g. `--cookie "sessionid=abc123; csrftoken=xyz"` — paste this
        /// from your browser's dev tools (Application/Storage tab), or use
        /// the Sprint 11 browser extension's cookie-import endpoint
        /// instead of copying it by hand.
        #[arg(long)]
        cookie: Option<String>,
        /// Sprint 12: resolve DNS for this download over HTTPS (DoH)
        /// instead of the system resolver. One of "cloudflare", "google",
        /// or "quad9". Falls back to plain DNS automatically if the DoH
        /// endpoint itself is unreachable.
        #[arg(long = "doh", value_name = "PROVIDER")]
        doh: Option<String>,
        /// Sprint 12: persist the `--header`/`--bearer`/`--cookie` given
        /// here as this download's domain's auth config, so future
        /// downloads from the same domain (with `sdm download`, no flags
        /// needed) pick it up automatically via
        /// `sdm_storage::auth::resolve_auth_config`. Stored encrypted —
        /// see `sdm_storage::auth`.
        #[arg(long = "save-auth")]
        save_auth: bool,
    },
    /// Resume a previously started job by ID.
    Resume {
        job_id: String,
        /// Same meaning as `sdm download`'s flags — needed again on
        /// resume because SSH credentials/host-key trust aren't persisted.
        #[arg(long = "ssh-password")]
        ssh_password: Option<String>,
        #[arg(long = "ssh-key")]
        ssh_key: Option<String>,
        #[arg(long = "ssh-key-passphrase")]
        ssh_key_passphrase: Option<String>,
        #[arg(long = "known-hosts")]
        known_hosts: Option<String>,
        #[arg(long = "accept-new-hostkey")]
        accept_new_hostkey: bool,
    },
    /// List all known jobs.
    List,
    /// Show details for one job.
    Show { job_id: String },
    /// List a remote directory's contents (`ftp(s)://`, `sftp://`, or
    /// `webdav(s)://`).
    ListRemote {
        url: String,
        #[arg(long = "ssh-password")]
        ssh_password: Option<String>,
        #[arg(long = "ssh-key")]
        ssh_key: Option<String>,
        #[arg(long = "ssh-key-passphrase")]
        ssh_key_passphrase: Option<String>,
        #[arg(long = "known-hosts")]
        known_hosts: Option<String>,
        #[arg(long = "accept-new-hostkey")]
        accept_new_hostkey: bool,
    },
    /// Re-verify a completed job's checksum and per-chunk hashes, and
    /// (with `--repair`) re-fetch only the chunks found to be corrupt.
    Verify {
        job_id: String,
        /// Compare against this checksum instead of (or in addition to)
        /// whatever was recorded at download time. Format: "algorithm:hex".
        #[arg(long)]
        checksum: Option<String>,
        /// Re-fetch any corrupt chunks found, instead of just reporting them.
        #[arg(long)]
        repair: bool,
    },
    /// Sprint 10: show a yt-dlp-backed source's title, duration, and
    /// available formats — use a format_id from here with
    /// `sdm download --via-ytdlp --media-quality <format_id> <url>`.
    Probe { url: String },
    /// Sprint 12: full-text + filtered search across download history and
    /// the active queue (filename, URL, category, status, date range).
    Search {
        /// Free-text query (matches filename, URL, category, status).
        /// Omit to just apply the filter flags below.
        text: Option<String>,
        /// Treat `text` as a regular expression instead of a full-text
        /// query. FTS5 has no native regex mode, so this path is matched
        /// in-process against the filtered candidate rows.
        #[arg(long)]
        regex: bool,
        #[arg(long)]
        category: Option<String>,
        /// One of: queued, probing, downloading, paused, verifying,
        /// completed, failed.
        #[arg(long)]
        status: Option<String>,
        /// Inclusive RFC3339 lower bound on `created_at`, e.g.
        /// "2026-01-01T00:00:00Z".
        #[arg(long = "from")]
        date_from: Option<String>,
        /// Inclusive RFC3339 upper bound on `created_at`.
        #[arg(long = "to")]
        date_to: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: i64,
    },
    /// Sprint 12: set the global default proxy (used by any download
    /// that doesn't pass its own `--proxy`). Credentials are stored
    /// encrypted — see `sdm_storage::credentials`.
    SetProxy {
        /// e.g. `socks5h://host:port`, `socks4://host:port`,
        /// `http://host:port`.
        url: String,
        #[arg(long = "user")]
        user: Option<String>,
        #[arg(long = "pass")]
        pass: Option<String>,
    },
    /// Sprint 12: clear the global default proxy (and delete its stored
    /// credential, if any).
    ClearProxy,
    /// Sprint 12: show the currently configured global default proxy
    /// (URL only — the credential itself is never printed).
    ShowProxy,
    /// Sprint 12: save a custom header (bearer token, API key, ...) for a
    /// domain or a specific job, applied automatically to future
    /// downloads/resumes that match. A job-scoped header overrides a
    /// domain-scoped one for that job.
    SetAuthHeader {
        /// Domain (e.g. "example.com") or, with `--job`, a job ID.
        scope: String,
        /// Header name, e.g. "Authorization" or "X-API-Key".
        name: String,
        value: String,
        /// Treat `scope` as a job ID instead of a domain.
        #[arg(long)]
        job: bool,
    },
    /// Sprint 12: save a raw `Cookie:` header value for a domain or job —
    /// same scoping rules as `set-auth-header`. Paste the value from your
    /// browser's dev tools, or (once paired — see `sdm pair`) let the
    /// browser extension import it via `POST /auth/cookies`.
    SetAuthCookie {
        scope: String,
        cookie: String,
        #[arg(long)]
        job: bool,
    },
    /// Sprint 12: show whether an auth config exists for a domain/job
    /// (never prints the header values/cookie themselves).
    ShowAuth {
        scope: String,
        #[arg(long)]
        job: bool,
    },
    /// Sprint 12: delete a saved auth config (and its encrypted
    /// credential) for a domain/job.
    ClearAuth {
        scope: String,
        #[arg(long)]
        job: bool,
    },
    /// Sprint 12: run the OAuth2 authorization-code flow (with PKCE) for
    /// `domain` and store the resulting access/refresh tokens, encrypted.
    /// Opens a local server on 127.0.0.1 to receive the provider's
    /// redirect — visit the printed URL in a browser to complete login.
    OauthLogin {
        /// Domain the resulting token should be applied to (matches the
        /// same domain-scoping `set-auth-header`/`set-auth-cookie` use).
        domain: String,
        #[arg(long = "auth-url")]
        auth_url: String,
        #[arg(long = "token-url")]
        token_url: String,
        #[arg(long = "client-id")]
        client_id: String,
        #[arg(long = "client-secret")]
        client_secret: Option<String>,
        /// Space- or comma-separated OAuth scopes to request.
        #[arg(long)]
        scope: Option<String>,
        /// Local port to receive the redirect on. A free port is chosen
        /// automatically if omitted.
        #[arg(long = "redirect-port")]
        redirect_port: Option<u16>,
    },
    /// Sprint 12: show the VPN-detection heuristic's current view of
    /// which network interfaces look VPN-like right now.
    VpnStatus,
    /// Sprint 12: run the VPN-detection monitor in the foreground,
    /// pausing active downloads if a VPN interface appears mid-session.
    /// Runs until interrupted (Ctrl-C). `sdmd` (the daemon) runs this
    /// automatically in the background — this is for standalone `sdm`
    /// use without the daemon.
    VpnWatch {
        /// Poll interval in seconds.
        #[arg(long, default_value_t = 5)]
        interval: u64,
    },
}

/// Encodes a username/password pair as the single string
/// `CredentialStore` stores per reference (it's a generic secret-string
/// store, not proxy-specific) — `\n` is a safe separator since neither
/// half of a proxy credential can legitimately contain a literal
/// newline. Kept as a named pair of functions rather than inlined at
/// both call sites (`SetProxy` and the global-proxy-resolving path
/// below) so the format only has one place to get wrong.
fn encode_proxy_credential(user: &str, pass: &str) -> String {
    format!("{user}\n{pass}")
}

fn decode_proxy_credential(stored: &str) -> Option<(String, String)> {
    stored
        .split_once('\n')
        .map(|(u, p)| (u.to_string(), p.to_string()))
}

/// Sprint 12: resolve the persisted global default proxy (`sdm set-proxy`),
/// if any, into a ready-to-use `ProxyConfig` (credential decrypted).
/// Factored out of `build_engine_with_global_proxy` so the `Download`
/// command can compose it with per-download auth headers/cookies/DNS mode
/// into one `ClientConfig` instead of only being able to use it in
/// isolation.
async fn resolve_global_proxy(
    pool: &sdm_storage::SqlitePool,
) -> anyhow::Result<Option<sdm_protocols::ProxyConfig>> {
    let Some(settings) = sdm_storage::get_global_proxy(pool).await? else {
        return Ok(None);
    };
    let mut cfg = sdm_protocols::ProxyConfig::new(settings.url);
    if let Some(credential_ref) = &settings.credential_ref {
        let store = sdm_storage::CredentialStore::new(pool.clone());
        let stored = store.retrieve(credential_ref).await?;
        if let Some((user, pass)) = decode_proxy_credential(&stored) {
            cfg = cfg.with_auth(user, pass);
        }
    }
    Ok(Some(cfg))
}

/// Sprint 12: build the shared `Engine` used by every CLI command,
/// honoring the persisted global default proxy (`sdm set-proxy`) if one
/// is configured. Per-download `--proxy` (see the `Commands::Download`
/// arm) overrides this on a per-invocation basis by constructing its own
/// `Engine::new_with_config` locally instead of using this one.
async fn build_engine_with_global_proxy(pool: &sdm_storage::SqlitePool) -> anyhow::Result<Engine> {
    match resolve_global_proxy(pool).await? {
        Some(cfg) => Ok(Engine::new_with_proxy(pool.clone(), Some(&cfg))?),
        None => Ok(Engine::new(pool.clone())),
    }
}

fn sdm_home() -> PathBuf {
    if let Ok(p) = std::env::var("SDM_HOME") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".sdm")
}

/// Metalink documents are usually local `.metalink`/`.meta4` files or an
/// `http(s)://` URL ending the same way — same detection style as
/// `sdm_torrent::looks_like_torrent_source` for magnet/`.torrent`.
fn is_metalink_source(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.ends_with(".metalink") || path.ends_with(".meta4")
}

/// Best-effort recognition of a handful of well-known yt-dlp-supported
/// sites, so `sdm download <url>` works without `--via-ytdlp` for the
/// most common case. This is deliberately NOT exhaustive — yt-dlp itself
/// supports thousands of extractors we have no reliable way to enumerate
/// or keep in sync with here — so `--via-ytdlp` remains the reliable way
/// to reach anything not on this short list.
fn looks_like_ytdlp_site(url: &str) -> bool {
    const KNOWN_HOSTS: &[&str] = &[
        "youtube.com",
        "youtu.be",
        "vimeo.com",
        "dailymotion.com",
        "twitch.tv",
        "soundcloud.com",
        "tiktok.com",
        "x.com/",
        "twitter.com/",
        "facebook.com/watch",
        "reddit.com/r/",
    ];
    let lower = url.to_ascii_lowercase();
    KNOWN_HOSTS.iter().any(|h| lower.contains(h))
}

fn default_destination(url: &str) -> String {
    let name = url
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("download");
    let name = name.split(['?', '#']).next().unwrap_or(name);
    if name.is_empty() {
        "download".to_string()
    } else {
        name.to_string()
    }
}

/// Build [`SshConnectionOptions`] from the CLI's `--ssh-*`/`--known-hosts`/
/// `--accept-new-hostkey` flags, falling back to any `user:pass@` embedded
/// directly in the URL when no explicit credential flag was given.
fn ssh_connection_options(
    ssh_url: &SshUrl,
    ssh_password: Option<String>,
    ssh_key: Option<String>,
    ssh_key_passphrase: Option<String>,
    known_hosts: Option<String>,
    accept_new_hostkey: bool,
) -> anyhow::Result<SshConnectionOptions> {
    let auth = if let Some(path) = ssh_key {
        SshAuth::PrivateKey {
            path: PathBuf::from(path),
            passphrase: ssh_key_passphrase,
        }
    } else if let Some(password) = ssh_password {
        SshAuth::Password(password)
    } else if let Some(password) = &ssh_url.password {
        SshAuth::Password(password.clone())
    } else {
        anyhow::bail!(
            "no SSH credentials given — pass --ssh-key <path>, --ssh-password <pw>, \
             or embed user:pass@ in the URL"
        );
    };

    let known_hosts_path = known_hosts
        .map(PathBuf::from)
        .unwrap_or_else(sdm_protocols::ssh::default_known_hosts_path);
    let host_key_policy = if accept_new_hostkey {
        HostKeyPolicy::AcceptNew
    } else {
        HostKeyPolicy::Strict
    };

    Ok(SshConnectionOptions {
        auth,
        known_hosts_path,
        host_key_policy,
    })
}

/// Parse a `--header "Name: Value"` flag into its `(name, value)` parts.
fn parse_header_flag(spec: &str) -> anyhow::Result<(String, String)> {
    let (name, value) = spec
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("invalid --header {spec:?}, expected \"Name: Value\""))?;
    Ok((name.trim().to_string(), value.trim().to_string()))
}

/// Build the `AuthScope` a `set-auth-*`/`show-auth`/`clear-auth` CLI
/// invocation targets, given its `scope` positional and `--job` flag.
fn auth_scope_from(scope: String, job: bool) -> sdm_storage::auth::AuthScope {
    if job {
        sdm_storage::auth::AuthScope::Job(scope)
    } else {
        sdm_storage::auth::AuthScope::Domain(scope)
    }
}

/// Parse a `sdm download --doh <provider>` value into a `DnsMode`.
fn parse_doh_flag(provider: &str) -> anyhow::Result<sdm_protocols::DnsMode> {
    sdm_protocols::DohProvider::parse(provider)
        .map(sdm_protocols::DnsMode::Doh)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unknown --doh provider {provider:?}, expected cloudflare, google, or quad9"
            )
        })
}

/// Sprint 12: compose the [`sdm_protocols::ClientConfig`] a `sdm
/// download` invocation should use, from (in priority order, highest
/// first): explicit `--proxy`/`--header`/`--bearer`/`--cookie`/`--doh`
/// flags; the persisted global default proxy (`sdm set-proxy`); and the
/// persisted per-domain auth config (`sdm set-auth-header`/
/// `set-auth-cookie`) for `url`'s host, used only when *no* explicit
/// `--header`/`--bearer`/`--cookie` was given on this invocation (an
/// explicit flag always fully replaces the persisted config rather than
/// merging with it, so a one-off `--header` override can't accidentally
/// pick up a stale saved cookie too).
#[allow(clippy::too_many_arguments)]
async fn resolve_download_client_config(
    pool: &sdm_storage::SqlitePool,
    url: &str,
    proxy: Option<&str>,
    proxy_user: Option<&str>,
    proxy_pass: Option<&str>,
    headers: &[String],
    bearer: Option<&str>,
    cookie: Option<&str>,
    doh: Option<&str>,
) -> anyhow::Result<sdm_protocols::ClientConfig> {
    let mut cfg = sdm_protocols::ClientConfig {
        proxy: match proxy {
            Some(url) => {
                let mut p = sdm_protocols::ProxyConfig::new(url.to_string());
                if let (Some(u), Some(pw)) = (proxy_user, proxy_pass) {
                    p = p.with_auth(u.to_string(), pw.to_string());
                }
                Some(p)
            }
            None => resolve_global_proxy(pool).await?,
        },
        ..Default::default()
    };

    if let Some(provider) = doh {
        cfg.dns = parse_doh_flag(provider)?;
    }

    let explicit_headers = !headers.is_empty() || bearer.is_some();
    if explicit_headers {
        for h in headers {
            let (name, value) = parse_header_flag(h)?;
            cfg.extra_headers
                .push(sdm_protocols::AuthHeader::new(name, value));
        }
        if let Some(token) = bearer {
            cfg.extra_headers
                .push(sdm_protocols::AuthHeader::bearer(token.to_string()));
        }
    }
    if let Some(raw_cookie) = cookie {
        cfg = cfg.with_cookie(raw_cookie.to_string(), url)?;
    }

    if !explicit_headers && cookie.is_none() {
        let store = sdm_storage::CredentialStore::new(pool.clone());
        if let Some(saved) = sdm_storage::auth::resolve_auth_config(pool, &store, None, url).await?
        {
            for (name, value) in saved.headers {
                cfg.extra_headers
                    .push(sdm_protocols::AuthHeader::new(name, value));
            }
            if let Some(saved_cookie) = saved.cookie {
                cfg = cfg.with_cookie(saved_cookie, url)?;
            }
        }
    }

    Ok(cfg)
}

fn parse_file_indices(spec: &str) -> anyhow::Result<Vec<usize>> {
    spec.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.parse::<usize>()
                .map_err(|_| anyhow::anyhow!("invalid file index in --only-files: {s:?}"))
        })
        .collect()
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let cli = Cli::parse();

    let home = sdm_home();
    tokio::fs::create_dir_all(&home).await.ok();
    let db_path = cli
        .db
        .unwrap_or_else(|| home.join("jobs.db").to_string_lossy().to_string());
    let pool = sdm_storage::connect(&db_path).await?;
    let engine = build_engine_with_global_proxy(&pool).await?;

    match cli.command {
        Commands::Download {
            url,
            output,
            connections,
            mirrors,
            checksum,
            on_duplicate,
            sequential,
            only_files,
            ssh_password,
            ssh_key,
            ssh_key_passphrase,
            known_hosts,
            accept_new_hostkey,
            quality,
            via_ytdlp,
            media_quality,
            subs,
            embed_thumbnail,
            proxy,
            proxy_user,
            proxy_pass,
            headers,
            bearer,
            cookie,
            doh,
            save_auth,
        } => {
            // Sprint 12: --proxy/--header/--bearer/--cookie/--doh apply to
            // every transport in this arm that shares Engine's single
            // reqwest client (HTTP, WebDAV, DASH, HLS, Metalink — see
            // crates/engine/src/download.rs's `self.client` call sites).
            // FTP (suppaftp) and BitTorrent (librqbit) use their own
            // transports and don't go through this client yet — not
            // covered by these flags in this release; see
            // `Engine::new_with_config`'s doc comment.
            let client_cfg = resolve_download_client_config(
                &pool,
                &url,
                proxy.as_deref(),
                proxy_user.as_deref(),
                proxy_pass.as_deref(),
                &headers,
                bearer.as_deref(),
                cookie.as_deref(),
                doh.as_deref(),
            )
            .await?;

            if save_auth && (!headers.is_empty() || bearer.is_some() || cookie.is_some()) {
                let store = sdm_storage::CredentialStore::new(pool.clone());
                let mut saved = sdm_storage::auth::AuthConfig::default();
                for h in &headers {
                    saved.headers.push(parse_header_flag(h)?);
                }
                if let Some(token) = &bearer {
                    saved
                        .headers
                        .push(("Authorization".to_string(), format!("Bearer {token}")));
                }
                saved.cookie = cookie.clone();
                if let Some(host) = url::Url::parse(&url)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_string))
                {
                    sdm_storage::auth::set_auth_config(
                        &pool,
                        &store,
                        &sdm_storage::auth::AuthScope::Domain(host.clone()),
                        &saved,
                    )
                    .await?;
                    println!("Saved auth config for domain {host}");
                } else {
                    eprintln!("--save-auth: could not parse a domain out of {url:?}, not saved");
                }
            }

            let is_default_client_cfg = client_cfg.proxy.is_none()
                && matches!(client_cfg.dns, sdm_protocols::DnsMode::Plain)
                && client_cfg.extra_headers.is_empty()
                && client_cfg.cookie_header.is_none();
            let engine = if is_default_client_cfg {
                engine
            } else {
                sdm_engine::Engine::new_with_config(pool.clone(), &client_cfg)?
            };

            if via_ytdlp || looks_like_ytdlp_site(&url) {
                let destination_dir = PathBuf::from(output.unwrap_or_else(|| ".".to_string()));
                let subtitle_langs = subs
                    .as_deref()
                    .map(|s| {
                        s.split(',')
                            .map(|l| l.trim().to_string())
                            .filter(|l| !l.is_empty())
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let selector = if media_quality.eq_ignore_ascii_case("best") {
                    sdm_engine::QualitySelector::Best
                } else {
                    sdm_engine::QualitySelector::FormatId(media_quality)
                };

                let media_engine = sdm_engine::MediaEngine::new(&pool);
                let (tx, rx) = sdm_engine::channel();
                let bar_task = tokio::spawn(render_progress(rx));
                let req = sdm_engine::MediaDownloadRequest {
                    url,
                    destination_dir,
                    quality: selector,
                    subtitle_langs,
                    embed_thumbnail,
                    duplicate_policy: DuplicatePolicy::parse(&on_duplicate)?,
                    ytdlp: sdm_media::YtDlpBinary::default(),
                    ffmpeg: sdm_media::FfmpegBinary::default(),
                };
                let result = media_engine.start_download(req, tx).await;
                let _ = bar_task.await;

                match result {
                    Ok(job) => println!("✓ Downloaded to {}", job.destination),
                    Err(e) => {
                        eprintln!("✗ Media download failed: {e}");
                        std::process::exit(1);
                    }
                }
            } else if sdm_torrent::looks_like_torrent_source(&url) {
                let destination_folder = PathBuf::from(output.unwrap_or_else(|| ".".to_string()));
                let only_files = only_files.as_deref().map(parse_file_indices).transpose()?;

                let (tx, rx) = sdm_engine::channel();
                let bar_task = tokio::spawn(render_progress(rx));
                let req = TorrentDownloadRequest {
                    source: url,
                    destination_folder,
                    only_files,
                    sequential,
                };
                let result = engine.start_torrent_download(req, tx).await;
                let _ = bar_task.await;

                match result {
                    Ok(job) => println!("✓ Downloaded to {}", job.destination),
                    Err(e) => {
                        eprintln!("✗ Torrent download failed: {e}");
                        std::process::exit(1);
                    }
                }
            } else if is_metalink_source(&url) {
                let destination_dir = PathBuf::from(output.unwrap_or_else(|| ".".to_string()));
                let connections_opt = ConnectionsOption::parse(&connections)?;
                let duplicate_policy = DuplicatePolicy::parse(&on_duplicate)?;

                let (tx, rx) = sdm_engine::channel();
                let bar_task = tokio::spawn(render_progress(rx));
                let source = sdm_engine::MetalinkSource::parse(&url);
                let result = engine
                    .start_metalink_download(
                        source,
                        destination_dir,
                        connections_opt,
                        duplicate_policy,
                        tx,
                    )
                    .await;
                let _ = bar_task.await;

                match result {
                    Ok(job) => println!("✓ Downloaded to {}", job.destination),
                    Err(e) => {
                        eprintln!("✗ Metalink download failed: {e}");
                        std::process::exit(1);
                    }
                }
            } else if url.contains(".m3u8") {
                let destination =
                    PathBuf::from(output.unwrap_or_else(|| default_destination(&url)));
                let variant = sdm_protocols::hls::VariantSelector::parse(&quality)?;
                let checksum = checksum
                    .as_deref()
                    .map(ExpectedChecksum::parse)
                    .transpose()?;

                let (tx, rx) = sdm_engine::channel();
                let bar_task = tokio::spawn(render_progress(rx));
                let req = HlsDownloadRequest {
                    url,
                    destination,
                    variant,
                    expected_checksum: checksum,
                    duplicate_policy: DuplicatePolicy::parse(&on_duplicate)?,
                    max_live_polls: None,
                };
                let result = engine.start_hls_download(req, tx).await;
                let _ = bar_task.await;

                match result {
                    Ok(job) => println!("✓ Downloaded to {}", job.destination),
                    Err(e) => {
                        eprintln!("✗ HLS download failed: {e}");
                        std::process::exit(1);
                    }
                }
            } else if url.contains(".mpd") {
                let destination_dir = PathBuf::from(output.unwrap_or_else(|| ".".to_string()));
                let default_name = default_destination(&url);
                let file_stem = default_name
                    .rsplit_once('.')
                    .map(|(stem, _)| stem.to_string())
                    .unwrap_or(default_name);

                let (tx, rx) = sdm_engine::channel();
                let bar_task = tokio::spawn(render_progress(rx));
                let req = DashDownloadRequest {
                    url,
                    destination_dir,
                    file_stem,
                    duplicate_policy: DuplicatePolicy::parse(&on_duplicate)?,
                };
                let result = engine.start_dash_download(req, tx).await;
                let _ = bar_task.await;

                match result {
                    Ok(job) => println!(
                        "✓ Downloaded video to {} (audio alongside it, if the manifest had a separate audio track)",
                        job.destination
                    ),
                    Err(e) => {
                        eprintln!("✗ DASH download failed: {e}");
                        std::process::exit(1);
                    }
                }
            } else if url.starts_with("ftp://") || url.starts_with("ftps://") {
                let destination =
                    PathBuf::from(output.unwrap_or_else(|| default_destination(&url)));

                let (tx, rx) = sdm_engine::channel();
                let bar_task = tokio::spawn(render_progress(rx));
                let req = FtpDownloadRequest { url, destination };
                let result = engine.start_ftp_download(req, tx).await;
                let _ = bar_task.await;

                match result {
                    Ok(job) => println!("✓ Downloaded to {}", job.destination),
                    Err(e) => {
                        eprintln!("✗ FTP download failed: {e}");
                        std::process::exit(1);
                    }
                }
            } else if url.starts_with("sftp://") {
                let destination =
                    PathBuf::from(output.unwrap_or_else(|| default_destination(&url)));
                let ssh_url = SshUrl::parse(&url, "sftp")?;
                let connection = ssh_connection_options(
                    &ssh_url,
                    ssh_password,
                    ssh_key,
                    ssh_key_passphrase,
                    known_hosts,
                    accept_new_hostkey,
                )?;
                let connections = ConnectionsOption::parse(&connections)?;

                let ssh_engine = SshEngine::new(&pool);
                let (tx, rx) = sdm_engine::channel();
                let bar_task = tokio::spawn(render_progress(rx));
                let req = SftpDownloadRequest {
                    url,
                    destination,
                    connections,
                    connection,
                };
                let result = ssh_engine.start_sftp_download(req, tx).await;
                let _ = bar_task.await;

                match result {
                    Ok(job) => println!("✓ Downloaded to {}", job.destination),
                    Err(e) => {
                        eprintln!("✗ SFTP download failed: {e}");
                        std::process::exit(1);
                    }
                }
            } else if url.starts_with("scp://") {
                let destination =
                    PathBuf::from(output.unwrap_or_else(|| default_destination(&url)));
                let ssh_url = SshUrl::parse(&url, "scp")?;
                let connection = ssh_connection_options(
                    &ssh_url,
                    ssh_password,
                    ssh_key,
                    ssh_key_passphrase,
                    known_hosts,
                    accept_new_hostkey,
                )?;

                let ssh_engine = SshEngine::new(&pool);
                let (tx, rx) = sdm_engine::channel();
                let bar_task = tokio::spawn(render_progress(rx));
                let req = ScpDownloadRequest {
                    url,
                    destination,
                    connection,
                };
                let result = ssh_engine.start_scp_download(req, tx).await;
                let _ = bar_task.await;

                match result {
                    Ok(job) => println!("✓ Downloaded to {}", job.destination),
                    Err(e) => {
                        eprintln!("✗ SCP download failed: {e}");
                        std::process::exit(1);
                    }
                }
            } else if url.starts_with("webdav://") || url.starts_with("webdavs://") {
                let destination =
                    PathBuf::from(output.unwrap_or_else(|| default_destination(&url)));
                let connections = ConnectionsOption::parse(&connections)?;
                let expected_checksum = checksum
                    .as_deref()
                    .map(ExpectedChecksum::parse)
                    .transpose()?;

                let webdav_engine = WebDavEngine::new(&engine);
                let (tx, rx) = sdm_engine::channel();
                let bar_task = tokio::spawn(render_progress(rx));
                let req = WebDavDownloadRequest {
                    url,
                    destination,
                    connections,
                    expected_checksum,
                };
                let result = webdav_engine.start_download(req, tx).await;
                let _ = bar_task.await;

                match result {
                    Ok(job) => {
                        println!("✓ Downloaded to {}", job.destination);
                        if let Some(actual) = &job.checksum_actual {
                            let verified = if job.checksum_verified {
                                "verified"
                            } else {
                                "computed, not compared"
                            };
                            println!(
                                "  checksum ({}): {actual} [{verified}]",
                                job.checksum_algorithm.as_deref().unwrap_or("sha256")
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!("✗ WebDAV download failed: {e}");
                        std::process::exit(1);
                    }
                }
            } else {
                let destination =
                    PathBuf::from(output.unwrap_or_else(|| default_destination(&url)));
                let connections = ConnectionsOption::parse(&connections)?;
                let expected_checksum = checksum
                    .as_deref()
                    .map(ExpectedChecksum::parse)
                    .transpose()?;
                let duplicate_policy = DuplicatePolicy::parse(&on_duplicate)?;

                if duplicate_policy != DuplicatePolicy::Overwrite {
                    let dupes = engine
                        .check_duplicates(
                            &url,
                            &destination,
                            expected_checksum.as_ref().map(|c| c.hex.as_str()),
                        )
                        .await?;
                    if let Some(existing) = dupes.first() {
                        match duplicate_policy {
                            DuplicatePolicy::Skip => {
                                println!(
                                    "⚠ Skipping: looks like a duplicate of job {} ({})",
                                    existing.job.id, existing.job.destination
                                );
                                return Ok(());
                            }
                            DuplicatePolicy::Rename => {
                                println!(
                                    "⚠ Duplicate of job {} detected ({}); saving alongside it as a new file",
                                    existing.job.id, existing.job.destination
                                );
                            }
                            DuplicatePolicy::Overwrite => unreachable!(),
                        }
                    }
                }

                let (tx, rx) = sdm_engine::channel();
                let bar_task = tokio::spawn(render_progress(rx));

                let req = DownloadRequest {
                    url,
                    mirrors,
                    destination,
                    connections,
                    expected_checksum,
                    duplicate_policy,
                };
                let result = engine.start_download(req, tx).await;
                let _ = bar_task.await;

                match result {
                    Ok(job) => {
                        println!("✓ Downloaded to {}", job.destination);
                        if let Some(actual) = &job.checksum_actual {
                            let verified = if job.checksum_verified {
                                "verified"
                            } else {
                                "computed, not compared"
                            };
                            println!(
                                "  checksum ({}): {actual} [{verified}]",
                                job.checksum_algorithm.as_deref().unwrap_or("sha256")
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!("✗ Download failed: {e}");
                        std::process::exit(1);
                    }
                }
            }
        }
        Commands::Resume {
            job_id,
            ssh_password,
            ssh_key,
            ssh_key_passphrase,
            known_hosts,
            accept_new_hostkey,
        } => {
            let kind = sdm_storage::get_job(&pool, &job_id)
                .await?
                .map(|j| j.job_kind)
                .unwrap_or(sdm_storage::JobKind::Http);

            // Sprint 12: a resumed HTTP/WebDAV job whose domain has a
            // saved auth config (`sdm set-auth-header`/`set-auth-cookie`,
            // or one saved via `--save-auth` on the original `download`)
            // needs that auth applied again — SSH-style flags are
            // re-prompted on resume (see this match's other arms)
            // because SSH credentials aren't persisted at all, but HTTP
            // auth *is* persisted, so it's resolved automatically here
            // rather than requiring the same flags all over again.
            let engine = if matches!(
                kind,
                sdm_storage::JobKind::Http | sdm_storage::JobKind::WebDav
            ) {
                if let Some(job) = sdm_storage::get_job(&pool, &job_id).await? {
                    let store = sdm_storage::CredentialStore::new(pool.clone());
                    match sdm_storage::auth::resolve_auth_config(
                        &pool,
                        &store,
                        Some(&job_id),
                        &job.url,
                    )
                    .await?
                    {
                        Some(saved) if !saved.is_empty() => {
                            let mut cfg = sdm_protocols::ClientConfig {
                                proxy: resolve_global_proxy(&pool).await?,
                                ..Default::default()
                            };
                            for (name, value) in saved.headers {
                                cfg.extra_headers
                                    .push(sdm_protocols::AuthHeader::new(name, value));
                            }
                            if let Some(cookie) = saved.cookie {
                                cfg = cfg.with_cookie(cookie, &job.url)?;
                            }
                            sdm_engine::Engine::new_with_config(pool.clone(), &cfg)?
                        }
                        _ => engine,
                    }
                } else {
                    engine
                }
            } else {
                engine
            };

            let (tx, rx) = sdm_engine::channel();
            let bar_task = tokio::spawn(render_progress(rx));
            let result = match kind {
                sdm_storage::JobKind::Http => engine.resume_download(&job_id, tx).await,
                sdm_storage::JobKind::Ftp => engine.resume_ftp_download(job_id.clone(), tx).await,
                sdm_storage::JobKind::Torrent => {
                    engine.resume_torrent_download(job_id.clone(), tx).await
                }
                sdm_storage::JobKind::Hls => engine.resume_hls_download(job_id.clone(), tx).await,
                sdm_storage::JobKind::Dash => engine.resume_dash_download(job_id.clone(), tx).await,
                sdm_storage::JobKind::WebDav => {
                    WebDavEngine::new(&engine)
                        .resume_download(&job_id, tx)
                        .await
                }
                sdm_storage::JobKind::Sftp | sdm_storage::JobKind::Scp => {
                    let record = sdm_storage::get_job(&pool, &job_id)
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("job {job_id} not found"))?;
                    let scheme = if kind == sdm_storage::JobKind::Sftp {
                        "sftp"
                    } else {
                        "scp"
                    };
                    let ssh_url = SshUrl::parse(&record.url, scheme)?;
                    let connection = ssh_connection_options(
                        &ssh_url,
                        ssh_password,
                        ssh_key,
                        ssh_key_passphrase,
                        known_hosts,
                        accept_new_hostkey,
                    )?;
                    let ssh_engine = SshEngine::new(&pool);
                    if kind == sdm_storage::JobKind::Sftp {
                        ssh_engine
                            .resume_sftp_download(job_id.clone(), connection, tx)
                            .await
                    } else {
                        ssh_engine
                            .resume_scp_download(job_id.clone(), connection, tx)
                            .await
                    }
                }
                sdm_storage::JobKind::Media => {
                    let media_engine = sdm_engine::MediaEngine::new(&pool);
                    media_engine
                        .resume_download(
                            job_id.clone(),
                            sdm_media::YtDlpBinary::default(),
                            sdm_media::FfmpegBinary::default(),
                            tx,
                        )
                        .await
                }
            };
            let _ = bar_task.await;

            match result {
                Ok(job) => println!("✓ Resumed and completed: {}", job.destination),
                Err(e) => {
                    eprintln!("✗ Resume failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::List => {
            let jobs = sdm_storage::list_jobs(&pool).await?;
            if jobs.is_empty() {
                println!("No jobs yet.");
            }
            for j in jobs {
                println!(
                    "{}  {:<8}  {:<12}  {:>3}%  {}",
                    j.id,
                    j.job_kind.as_str(),
                    j.status.as_str(),
                    progress_pct(j.downloaded_bytes, j.total_bytes),
                    j.url
                );
            }
        }
        Commands::Show { job_id } => match sdm_storage::get_job(&pool, &job_id).await? {
            Some(j) => {
                println!("id:          {}", j.id);
                println!("kind:        {}", j.job_kind.as_str());
                println!("url:         {}", j.url);
                println!("destination: {}", j.destination);
                println!("status:      {}", j.status.as_str());
                println!(
                    "progress:    {}%",
                    progress_pct(j.downloaded_bytes, j.total_bytes)
                );
                println!("connections: {}", j.connections);
                if j.job_kind == sdm_storage::JobKind::Torrent {
                    if let Some(meta) = sdm_storage::get_torrent_meta(&pool, &job_id).await? {
                        println!("info-hash:   {}", meta.info_hash);
                        if let Some(name) = &meta.display_name {
                            println!("name:        {name}");
                        }
                        println!("peers:       {}", meta.peer_count);
                        if let Some(pieces) = meta.piece_count {
                            println!("pieces:      {pieces}");
                        }
                    }
                }
                if matches!(
                    j.job_kind,
                    sdm_storage::JobKind::Hls | sdm_storage::JobKind::Dash
                ) {
                    if let Some(meta) = sdm_storage::get_manifest_meta(&pool, &job_id).await? {
                        println!("manifest:    {}", meta.manifest_url);
                        if let Some(variant) = &meta.selected_variant {
                            println!("variant:     {variant}");
                        }
                        if let Some(v) = &meta.video_representation_id {
                            println!("video rep:   {v}");
                        }
                        if let Some(a) = &meta.audio_representation_id {
                            println!("audio rep:   {a}");
                        }
                        if meta.is_live {
                            println!("live:        yes");
                        }
                    }
                }
                if let Some(algo) = &j.checksum_algorithm {
                    if let Some(actual) = &j.checksum_actual {
                        println!(
                            "checksum:    {algo}:{actual} ({})",
                            if j.checksum_verified {
                                "verified"
                            } else {
                                "unverified"
                            }
                        );
                    }
                }
                if let Some(e) = &j.error_message {
                    println!("error:       {e}");
                }
            }
            None => {
                eprintln!("no such job: {job_id}");
                std::process::exit(1);
            }
        },
        Commands::ListRemote {
            url,
            ssh_password,
            ssh_key,
            ssh_key_passphrase,
            known_hosts,
            accept_new_hostkey,
        } => {
            if url.starts_with("ftp://") || url.starts_with("ftps://") {
                let ftp_url = sdm_protocols::ftp::FtpUrl::parse(&url)?;
                let mut session = sdm_protocols::ftp::FtpSession::connect(&ftp_url).await?;
                let path = if ftp_url.path.is_empty() {
                    None
                } else {
                    Some(ftp_url.path.as_str())
                };
                let entries = session.list_dir(path).await?;
                for entry in entries {
                    println!("{entry}");
                }
            } else if url.starts_with("sftp://") {
                let ssh_url = SshUrl::parse(&url, "sftp")?;
                let connection = ssh_connection_options(
                    &ssh_url,
                    ssh_password,
                    ssh_key,
                    ssh_key_passphrase,
                    known_hosts,
                    accept_new_hostkey,
                )?;
                let session = sdm_protocols::ssh::SshSession::connect(
                    &ssh_url,
                    &connection.auth,
                    connection.known_hosts_path,
                    connection.host_key_policy,
                )
                .await?;
                let remote_path = ssh_url.path.clone();
                let entries = sdm_protocols::sftp::list_dir(&session, &remote_path).await?;
                for entry in entries {
                    let marker = if entry.is_dir { "/" } else { "" };
                    println!("{}{marker}  {}", entry.name, entry.size);
                }
            } else if url.starts_with("webdav://") || url.starts_with("webdavs://") {
                let client = sdm_protocols::build_client();
                let entries = sdm_protocols::webdav::list_dir(&client, &url).await?;
                for entry in entries {
                    let marker = if entry.is_collection { "/" } else { "" };
                    println!(
                        "{}{marker}  {}",
                        entry.href,
                        entry.content_length.unwrap_or(0)
                    );
                }
            } else {
                anyhow::bail!(
                    "list-remote only supports ftp(s)://, sftp://, and webdav(s):// URLs"
                );
            }
        }
        Commands::Verify {
            job_id,
            checksum,
            repair,
        } => {
            let record = match sdm_storage::get_job(&pool, &job_id).await? {
                Some(r) => r,
                None => {
                    eprintln!("no such job: {job_id}");
                    std::process::exit(1);
                }
            };

            if repair {
                let n = engine.verify_and_repair(&job_id).await?;
                if n == 0 {
                    println!("✓ No corrupt chunks found.");
                } else {
                    println!("✓ Repaired {n} corrupt chunk(s).");
                }
            } else {
                let destination = PathBuf::from(&record.destination);
                let corrupt = sdm_engine::find_corrupt_chunks(&pool, &job_id, &destination).await?;
                if corrupt.is_empty() {
                    println!("✓ All chunks match their recorded hashes.");
                } else {
                    println!("✗ {} corrupt chunk(s) found:", corrupt.len());
                    for c in &corrupt {
                        println!("  bytes {}-{}", c.start_byte, c.end_byte);
                    }
                    println!("  run with --repair to re-fetch them");
                }
            }

            if let Some(spec) = checksum {
                let expected = ExpectedChecksum::parse(&spec)?;
                let destination = PathBuf::from(&record.destination);
                let (actual, matches) = sdm_engine::verify_file(&destination, &expected).await?;
                if matches {
                    println!("✓ Checksum matches: {actual}");
                } else {
                    println!(
                        "✗ Checksum mismatch: expected {}, got {actual}",
                        expected.hex
                    );
                    std::process::exit(1);
                }
            }
        }
        Commands::Probe { url } => {
            let ytdlp = sdm_media::YtDlpClient::new(sdm_media::YtDlpBinary::default());
            let metadata = ytdlp.probe(&url).await?;

            println!("{}", metadata.title.as_deref().unwrap_or("(untitled)"));
            if let Some(d) = metadata.duration {
                println!("  duration: {:.0}s", d);
            }
            if metadata.is_livestream() {
                println!("  ⚠ this is a livestream (will use --live-from-start)");
            }
            if !metadata.chapters.is_empty() {
                println!("  {} chapter(s)", metadata.chapters.len());
            }
            println!("  formats:");
            for f in &metadata.formats {
                println!(
                    "    {:<12} {:<8} video={} audio={}",
                    f.format_id,
                    f.quality_label(),
                    f.has_video(),
                    f.has_audio()
                );
            }
        }
        Commands::Search {
            text,
            regex,
            category,
            status,
            date_from,
            date_to,
            limit,
        } => {
            let status = status
                .as_deref()
                .map(|s| s.parse::<sdm_storage::JobStatus>())
                .transpose()?;
            let query = sdm_storage::SearchQuery {
                text,
                regex,
                category,
                status,
                date_from,
                date_to,
                limit: Some(limit),
            };
            let started = std::time::Instant::now();
            let results = sdm_storage::search_jobs(&pool, &query).await?;
            if results.is_empty() {
                println!("No matches.");
            }
            for r in &results {
                println!(
                    "{}  {:<8}  {:<12}  {:<12}  {}",
                    r.job_id,
                    r.job_kind.as_str(),
                    r.status.as_str(),
                    r.category.as_deref().unwrap_or("-"),
                    r.filename,
                );
            }
            eprintln!(
                "{} match(es) in {:.1}ms",
                results.len(),
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
        Commands::SetProxy { url, user, pass } => {
            // Validate the URL is actually usable before persisting it —
            // catches a typo'd scheme immediately instead of only at the
            // next download.
            sdm_protocols::build_client_with_proxy(Some(&sdm_protocols::ProxyConfig::new(
                url.clone(),
            )))?;

            let credential_ref = match (user, pass) {
                (Some(u), Some(p)) => {
                    let store = sdm_storage::CredentialStore::new(pool.clone());
                    Some(store.store(&encode_proxy_credential(&u, &p)).await?)
                }
                (None, None) => None,
                _ => {
                    eprintln!("--user and --pass must be given together (or neither, for an unauthenticated proxy)");
                    std::process::exit(1);
                }
            };
            sdm_storage::set_global_proxy(
                &pool,
                Some(&sdm_storage::ProxySettings {
                    url,
                    credential_ref,
                }),
            )
            .await?;
            println!("Global proxy set.");
        }
        Commands::ClearProxy => {
            if let Some(existing) = sdm_storage::get_global_proxy(&pool).await? {
                if let Some(credential_ref) = existing.credential_ref {
                    let store = sdm_storage::CredentialStore::new(pool.clone());
                    store.delete(&credential_ref).await?;
                }
            }
            sdm_storage::set_global_proxy(&pool, None).await?;
            println!("Global proxy cleared.");
        }
        Commands::ShowProxy => match sdm_storage::get_global_proxy(&pool).await? {
            Some(settings) => println!(
                "{}{}",
                settings.url,
                if settings.credential_ref.is_some() {
                    " (authenticated)"
                } else {
                    ""
                }
            ),
            None => println!("No global proxy configured."),
        },
        Commands::SetAuthHeader {
            scope,
            name,
            value,
            job,
        } => {
            let store = sdm_storage::CredentialStore::new(pool.clone());
            let auth_scope = auth_scope_from(scope.clone(), job);
            let mut cfg = sdm_storage::auth::get_auth_config(&pool, &store, &auth_scope)
                .await?
                .unwrap_or_default();
            cfg.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
            cfg.headers.push((name, value));
            sdm_storage::auth::set_auth_config(&pool, &store, &auth_scope, &cfg).await?;
            println!(
                "Saved header for {} {scope:?}.",
                if job { "job" } else { "domain" }
            );
        }
        Commands::SetAuthCookie { scope, cookie, job } => {
            let store = sdm_storage::CredentialStore::new(pool.clone());
            let auth_scope = auth_scope_from(scope.clone(), job);
            let mut cfg = sdm_storage::auth::get_auth_config(&pool, &store, &auth_scope)
                .await?
                .unwrap_or_default();
            cfg.cookie = Some(cookie);
            sdm_storage::auth::set_auth_config(&pool, &store, &auth_scope, &cfg).await?;
            println!(
                "Saved cookie for {} {scope:?}.",
                if job { "job" } else { "domain" }
            );
        }
        Commands::ShowAuth { scope, job } => {
            let store = sdm_storage::CredentialStore::new(pool.clone());
            let auth_scope = auth_scope_from(scope.clone(), job);
            match sdm_storage::auth::get_auth_config(&pool, &store, &auth_scope).await? {
                Some(cfg) => {
                    let header_names: Vec<&str> =
                        cfg.headers.iter().map(|(n, _)| n.as_str()).collect();
                    println!(
                        "Headers: {}{}",
                        if header_names.is_empty() {
                            "(none)".to_string()
                        } else {
                            header_names.join(", ")
                        },
                        if cfg.cookie.is_some() {
                            "; cookie: configured"
                        } else {
                            "; cookie: (none)"
                        }
                    );
                }
                None => println!("No auth config for this scope."),
            }
        }
        Commands::ClearAuth { scope, job } => {
            let store = sdm_storage::CredentialStore::new(pool.clone());
            let auth_scope = auth_scope_from(scope, job);
            sdm_storage::auth::delete_auth_config(&pool, &store, &auth_scope).await?;
            println!("Auth config cleared.");
        }
        Commands::OauthLogin {
            domain,
            auth_url,
            token_url,
            client_id,
            client_secret,
            scope,
            redirect_port,
        } => {
            run_oauth_login(
                &pool,
                domain,
                auth_url,
                token_url,
                client_id,
                client_secret,
                scope,
                redirect_port,
            )
            .await?;
        }
        Commands::VpnStatus => {
            let interfaces = sdm_engine::vpn::detect_vpn_interfaces();
            if interfaces.is_empty() {
                println!("No VPN-like interfaces detected.");
            } else {
                println!(
                    "VPN-like interfaces: {}",
                    interfaces.into_iter().collect::<Vec<_>>().join(", ")
                );
            }
        }
        Commands::VpnWatch { interval } => {
            println!("Watching for VPN interface changes every {interval}s (Ctrl-C to stop)...");
            sdm_engine::VpnMonitor::new(pool.clone())
                .with_poll_interval(std::time::Duration::from_secs(interval))
                .run()
                .await;
        }
    }

    Ok(())
}

/// Sprint 12: OAuth2 authorization-code flow with PKCE, driven entirely
/// from the CLI — prints the authorization URL for the person to open in
/// a browser, listens on a local loopback port for the provider's
/// redirect (`http://127.0.0.1:<port>/callback?code=...&state=...`),
/// exchanges the code for tokens, and stores them encrypted via
/// `sdm_storage::auth::store_oauth_tokens`.
#[allow(clippy::too_many_arguments)]
async fn run_oauth_login(
    pool: &sdm_storage::SqlitePool,
    domain: String,
    auth_url: String,
    token_url: String,
    client_id: String,
    client_secret: Option<String>,
    scope: Option<String>,
    redirect_port: Option<u16>,
) -> anyhow::Result<()> {
    use oauth2::basic::BasicClient;
    use oauth2::{
        AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge,
        RedirectUrl, Scope, TokenResponse, TokenUrl,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let bind_addr = format!("127.0.0.1:{}", redirect_port.unwrap_or(0));
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .map_err(|e| {
            anyhow::anyhow!("could not bind local redirect listener on {bind_addr}: {e}")
        })?;
    let port = listener.local_addr()?.port();
    let redirect_uri = RedirectUrl::new(format!("http://127.0.0.1:{port}/callback"))?;

    let client = BasicClient::new(
        ClientId::new(client_id),
        client_secret.map(ClientSecret::new),
        AuthUrl::new(auth_url)?,
        Some(TokenUrl::new(token_url)?),
    )
    .set_redirect_uri(redirect_uri);

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let mut auth_request = client
        .authorize_url(CsrfToken::new_random)
        .set_pkce_challenge(pkce_challenge);
    if let Some(scopes) = &scope {
        for s in scopes.split([',', ' ']).filter(|s| !s.is_empty()) {
            auth_request = auth_request.add_scope(Scope::new(s.to_string()));
        }
    }
    let (authorize_url, csrf_token) = auth_request.url();

    println!("Open this URL in a browser to log in:\n\n  {authorize_url}\n");
    println!("Waiting for the redirect on http://127.0.0.1:{port}/callback ...");

    let (mut stream, _) = listener.accept().await?;
    let mut buf = vec![0u8; 8192];
    let n = stream.read(&mut buf).await?;
    let request_text = String::from_utf8_lossy(&buf[..n]);
    let request_line = request_text.lines().next().unwrap_or("");
    let path_and_query = request_line.split_whitespace().nth(1).unwrap_or("");
    let full_url = format!("http://127.0.0.1:{port}{path_and_query}");
    let parsed = url::Url::parse(&full_url)
        .map_err(|e| anyhow::anyhow!("could not parse the OAuth redirect request line: {e}"))?;

    let mut code = None;
    let mut returned_state = None;
    for (k, v) in parsed.query_pairs() {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => returned_state = Some(v.into_owned()),
            _ => {}
        }
    }

    let body =
        "<html><body>Login complete — you can close this tab and return to the terminal.</body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;

    let code = code.ok_or_else(|| {
        anyhow::anyhow!("OAuth redirect had no ?code= — login likely failed or was cancelled")
    })?;
    match &returned_state {
        Some(s) if s.as_str() == csrf_token.secret().as_str() => {}
        _ => anyhow::bail!(
            "OAuth state mismatch on redirect — possible CSRF, aborting without storing any token"
        ),
    }

    let token_result = client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(pkce_verifier)
        .request_async(oauth2::reqwest::async_http_client)
        .await
        .map_err(|e| anyhow::anyhow!("token exchange failed: {e}"))?;

    let expires_at = token_result.expires_in().and_then(|d| {
        chrono::Duration::from_std(d)
            .ok()
            .map(|d| (chrono::Utc::now() + d).to_rfc3339())
    });
    let tokens = sdm_storage::auth::OAuthTokens {
        access_token: token_result.access_token().secret().clone(),
        refresh_token: token_result.refresh_token().map(|t| t.secret().clone()),
        // `BasicTokenType` (used by `BasicClient`) is Bearer in every
        // practical OAuth2 provider this flow targets; not worth
        // threading the type through just to re-stringify "Bearer".
        token_type: "Bearer".to_string(),
        expires_at,
    };
    let store = sdm_storage::CredentialStore::new(pool.clone());
    sdm_storage::auth::store_oauth_tokens(pool, &store, &domain, &tokens).await?;
    println!("OAuth2 login complete for {domain}; tokens stored encrypted.");
    Ok(())
}

fn progress_pct(downloaded: i64, total: Option<i64>) -> u32 {
    match total {
        Some(t) if t > 0 => ((downloaded as f64 / t as f64) * 100.0).min(100.0) as u32,
        _ => 0,
    }
}

async fn render_progress(mut rx: sdm_engine::ProgressReceiver) {
    let bar = ProgressBar::new(100);
    bar.set_style(
        ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("#>-"),
    );

    while let Some(event) = rx.recv().await {
        match event {
            ProgressEvent::Probing { .. } => bar.set_message("probing..."),
            ProgressEvent::Started {
                total_bytes,
                connections,
                ..
            } => {
                if let Some(total) = total_bytes {
                    bar.set_length(total);
                }
                bar.set_message(format!("downloading with {connections} connection(s)"));
            }
            ProgressEvent::Progress {
                downloaded_bytes,
                total_bytes,
                ..
            } => {
                if let Some(total) = total_bytes {
                    bar.set_length(total);
                }
                bar.set_position(downloaded_bytes);
            }
            ProgressEvent::Retrying {
                error_class,
                attempt,
                delay_ms,
                ..
            } => {
                bar.set_message(format!(
                    "retrying after {error_class} (attempt {attempt}, waiting {delay_ms}ms)"
                ));
            }
            ProgressEvent::Verifying { .. } => {
                bar.set_message("verifying checksum...");
            }
            ProgressEvent::Completed { .. } => {
                bar.finish_with_message("done");
            }
            ProgressEvent::Failed { message, .. } => {
                bar.abandon_with_message(format!("failed: {message}"));
            }
        }
    }
}
