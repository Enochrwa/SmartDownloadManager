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

/// Parse a `--only-files` value like `"0,2,5"` into file indices.
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
    let engine = Engine::new(pool.clone());

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
        } => {
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
                    // Sprint 10 scope: media jobs are a single yt-dlp
                    // invocation (yt-dlp resumes/continues a partial
                    // fetch itself by default), but this CLI doesn't yet
                    // re-drive that invocation for a job that was
                    // interrupted mid-`sdm download --via-ytdlp` — doing
                    // so honestly requires re-deriving the exact same
                    // format_id/subtitle/thumbnail request that was
                    // originally used, which isn't persisted anywhere
                    // beyond `media_meta.selected_format_id` yet. Rather
                    // than silently re-running with different
                    // (potentially wrong) options, this is a known,
                    // explicit gap for a future sprint.
                    eprintln!(
                        "✗ Resuming a media (yt-dlp) job isn't supported yet — re-run \
                         `sdm download --via-ytdlp <url>` instead."
                    );
                    std::process::exit(1);
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
    }

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
