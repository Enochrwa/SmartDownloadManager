//! Resume validation (Sprint 3).
//!
//! Before resuming a partially-downloaded job — possibly weeks later — we
//! re-probe the URL and check that the server-side resource hasn't changed
//! underneath us. If it has, we restart from scratch rather than splicing
//! new bytes into a now-stale partial file.

use sdm_protocols::ProbeInfo;
use sdm_storage::JobRecord;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeDecision {
    /// Validators match (or the server gave us nothing to compare and the
    /// size still matches) — safe to resume from the existing partial file.
    Resume,
    /// Validators disagree (or content-length shrank/changed) — the
    /// server-side resource changed, restart from scratch.
    Restart,
}

/// Compare what we stored from the original probe against a fresh probe
/// taken just before resuming.
pub fn decide_resume(stored: &JobRecord, fresh: &ProbeInfo) -> ResumeDecision {
    // Strongest signal: ETag. If we have one stored and the fresh one
    // differs (or is missing), the resource changed.
    if let Some(stored_etag) = &stored.etag {
        return match &fresh.etag {
            Some(fresh_etag) if fresh_etag == stored_etag => ResumeDecision::Resume,
            _ => ResumeDecision::Restart,
        };
    }

    // Next best: Last-Modified.
    if let Some(stored_lm) = &stored.last_modified {
        return match &fresh.last_modified {
            Some(fresh_lm) if fresh_lm == stored_lm => ResumeDecision::Resume,
            _ => ResumeDecision::Restart,
        };
    }

    // No validators at all — fall back to comparing total size. If it
    // changed, something about the resource changed; if it matches (or
    // both are unknown), we cautiously allow resume.
    match (stored.total_bytes, fresh.total_bytes) {
        (Some(a), Some(b)) if a as u64 == b => ResumeDecision::Resume,
        (Some(_), Some(_)) => ResumeDecision::Restart,
        _ => ResumeDecision::Resume,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(etag: Option<&str>, last_modified: Option<&str>, total: Option<i64>) -> JobRecord {
        JobRecord {
            id: "j1".into(),
            url: "https://example.com/f".into(),
            destination: "/tmp/f".into(),
            status: sdm_storage::JobStatus::Paused,
            total_bytes: total,
            downloaded_bytes: 0,
            connections: 4,
            supports_range: true,
            etag: etag.map(String::from),
            last_modified: last_modified.map(String::from),
            error_class: None,
            error_message: None,
            checksum_algorithm: None,
            checksum_expected: None,
            checksum_actual: None,
            checksum_verified: false,
        }
    }

    fn probe(etag: Option<&str>, last_modified: Option<&str>, total: Option<u64>) -> ProbeInfo {
        ProbeInfo {
            total_bytes: total,
            supports_range: true,
            etag: etag.map(String::from),
            last_modified: last_modified.map(String::from),
        }
    }

    #[test]
    fn matching_etag_resumes() {
        let stored = job(Some("\"abc\""), None, Some(1000));
        let fresh = probe(Some("\"abc\""), None, Some(1000));
        assert_eq!(decide_resume(&stored, &fresh), ResumeDecision::Resume);
    }

    #[test]
    fn changed_etag_restarts() {
        let stored = job(Some("\"abc\""), None, Some(1000));
        let fresh = probe(Some("\"xyz\""), None, Some(1000));
        assert_eq!(decide_resume(&stored, &fresh), ResumeDecision::Restart);
    }

    #[test]
    fn falls_back_to_last_modified_when_no_etag() {
        let stored = job(None, Some("Mon, 01 Jan 2024 00:00:00 GMT"), Some(1000));
        let matching = probe(None, Some("Mon, 01 Jan 2024 00:00:00 GMT"), Some(1000));
        let changed = probe(None, Some("Tue, 02 Jan 2024 00:00:00 GMT"), Some(1000));
        assert_eq!(decide_resume(&stored, &matching), ResumeDecision::Resume);
        assert_eq!(decide_resume(&stored, &changed), ResumeDecision::Restart);
    }

    #[test]
    fn falls_back_to_size_with_no_validators() {
        let stored = job(None, None, Some(1000));
        let same_size = probe(None, None, Some(1000));
        let different_size = probe(None, None, Some(2000));
        assert_eq!(decide_resume(&stored, &same_size), ResumeDecision::Resume);
        assert_eq!(
            decide_resume(&stored, &different_size),
            ResumeDecision::Restart
        );
    }

    #[test]
    fn no_information_at_all_cautiously_resumes() {
        let stored = job(None, None, None);
        let fresh = probe(None, None, None);
        assert_eq!(decide_resume(&stored, &fresh), ResumeDecision::Resume);
    }

    #[test]
    fn resume_weeks_later_with_unchanged_etag() {
        // Simulates: job paused/crashed, app restarted long after, etag on
        // the server is still the same (file never changed) -> resume.
        let stored = job(
            Some("\"stable-etag\""),
            Some("Mon, 01 Jan 2024 00:00:00 GMT"),
            Some(50_000_000),
        );
        let fresh = probe(
            Some("\"stable-etag\""),
            Some("Mon, 01 Jan 2024 00:00:00 GMT"),
            Some(50_000_000),
        );
        assert_eq!(decide_resume(&stored, &fresh), ResumeDecision::Resume);
    }
}
