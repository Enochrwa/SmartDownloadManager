//! Intelligent retry: per-error-class backoff policy.
//!
//! Sprint 3 scope: classify failures (done in `sdm_protocols::error`) and
//! decide, per class, how long to wait before retrying and how many
//! attempts are worth making before giving up.

use std::time::Duration;

use rand::Rng;
use sdm_protocols::ErrorClass;

/// How many attempts (including the first) we're willing to make for a
/// given error class before giving up on a segment.
pub fn max_attempts(class: &ErrorClass) -> u32 {
    match class {
        ErrorClass::ServerBusy { .. } => 8,
        ErrorClass::Timeout | ErrorClass::Other => 6,
        ErrorClass::DnsFailure | ErrorClass::TlsFailure => 4,
        ErrorClass::HttpError(500 | 502 | 503 | 504 | 408 | 425) => 5,
        ErrorClass::HttpError(_) => 1, // non-transient HTTP errors (404, 403, ...) aren't worth retrying
    }
}

/// How long to wait before the next attempt. `attempt` is 1-indexed (this
/// is the delay *before* attempt number `attempt + 1`).
pub fn backoff_delay(class: &ErrorClass, attempt: u32) -> Duration {
    match class {
        ErrorClass::ServerBusy {
            retry_after: Some(d),
        } => *d,
        ErrorClass::ServerBusy { retry_after: None } => exponential(attempt, 1_000, 60_000),
        ErrorClass::Timeout
        | ErrorClass::DnsFailure
        | ErrorClass::TlsFailure
        | ErrorClass::Other => exponential(attempt, 500, 30_000),
        ErrorClass::HttpError(_) => exponential(attempt, 1_000, 30_000),
    }
}

/// Exponential backoff with +/-20% jitter, in milliseconds, capped at `cap_ms`.
fn exponential(attempt: u32, base_ms: u64, cap_ms: u64) -> Duration {
    let exp = base_ms.saturating_mul(1u64 << attempt.saturating_sub(1).min(10));
    let capped = exp.min(cap_ms);
    let jitter_range = (capped / 5).max(1); // +/-20%
    let jitter = rand::thread_rng().gen_range(0..=jitter_range);
    Duration::from_millis(capped + jitter)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_busy_honors_retry_after_exactly() {
        let class = ErrorClass::ServerBusy {
            retry_after: Some(Duration::from_secs(7)),
        };
        assert_eq!(backoff_delay(&class, 1), Duration::from_secs(7));
        assert_eq!(backoff_delay(&class, 5), Duration::from_secs(7));
    }

    #[test]
    fn timeout_backoff_grows_with_attempts() {
        let class = ErrorClass::Timeout;
        let d1 = backoff_delay(&class, 1).as_millis();
        let d3 = backoff_delay(&class, 3).as_millis();
        let d6 = backoff_delay(&class, 6).as_millis();
        assert!(
            d3 > d1,
            "later attempts should back off further: {d1} vs {d3}"
        );
        assert!(d6 > d3);
    }

    #[test]
    fn backoff_is_capped() {
        let class = ErrorClass::Timeout;
        let d = backoff_delay(&class, 50).as_millis();
        assert!(d <= 30_000 + 30_000 / 5);
    }

    #[test]
    fn non_transient_http_errors_get_one_attempt() {
        assert_eq!(max_attempts(&ErrorClass::HttpError(404)), 1);
        assert_eq!(max_attempts(&ErrorClass::HttpError(403)), 1);
    }

    #[test]
    fn transient_http_errors_get_retried() {
        assert!(max_attempts(&ErrorClass::HttpError(503)) > 1);
        assert!(max_attempts(&ErrorClass::HttpError(500)) > 1);
    }

    #[test]
    fn dns_and_tls_failures_get_a_modest_retry_budget() {
        assert_eq!(max_attempts(&ErrorClass::DnsFailure), 4);
        assert_eq!(max_attempts(&ErrorClass::TlsFailure), 4);
    }
}
