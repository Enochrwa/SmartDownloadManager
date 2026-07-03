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

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use sdm_engine::{
    ConnectionsOption, DownloadRequest, DuplicatePolicy, Engine, ExpectedChecksum, ProgressEvent,
};

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
    },
    /// Resume a previously started job by ID.
    Resume { job_id: String },
    /// List all known jobs.
    List,
    /// Show details for one job.
    Show { job_id: String },
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
        } => {
            let destination = PathBuf::from(output.unwrap_or_else(|| default_destination(&url)));
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
        Commands::Resume { job_id } => {
            let (tx, rx) = sdm_engine::channel();
            let bar_task = tokio::spawn(render_progress(rx));
            let result = engine.resume_download(&job_id, tx).await;
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
                    "{}  {:<12}  {:>3}%  {}",
                    j.id,
                    j.status.as_str(),
                    progress_pct(j.downloaded_bytes, j.total_bytes),
                    j.url
                );
            }
        }
        Commands::Show { job_id } => match sdm_storage::get_job(&pool, &job_id).await? {
            Some(j) => {
                println!("id:          {}", j.id);
                println!("url:         {}", j.url);
                println!("destination: {}", j.destination);
                println!("status:      {}", j.status.as_str());
                println!(
                    "progress:    {}%",
                    progress_pct(j.downloaded_bytes, j.total_bytes)
                );
                println!("connections: {}", j.connections);
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
