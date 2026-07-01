//! Error classification for the retry state machine (Sprint 3).
//!
//! `crates/engine` owns the retry/backoff *policy*; this module's job is
//! purely to turn a transport failure or an HTTP response into one of a
//! small number of classes the policy can reason about.

use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorClass {
    /// Connect or read timeout.
    Timeout,
    /// DNS resolution failed.
    DnsFailure,
    /// TLS handshake / certificate failure.
    TlsFailure,
    /// 429 Too Many Requests or 503 Service Unavailable, optionally with a
    /// server-supplied `Retry-After`.
    ServerBusy { retry_after: Option<Duration> },
    /// Any other non-2xx HTTP status.
    HttpError(u16),
    /// Anything else (local I/O, protocol violations, etc).
    Other,
}

impl ErrorClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorClass::Timeout => "timeout",
            ErrorClass::DnsFailure => "dns_failure",
            ErrorClass::TlsFailure => "tls_failure",
            ErrorClass::ServerBusy { .. } => "server_busy",
            ErrorClass::HttpError(_) => "http_error",
            ErrorClass::Other => "other",
        }
    }

    /// Whether this class of failure is worth retrying at all. A 404 or
    /// other non-transient 4xx is not.
    pub fn is_retryable(&self) -> bool {
        match self {
            ErrorClass::Timeout
            | ErrorClass::DnsFailure
            | ErrorClass::TlsFailure
            | ErrorClass::Other => true,
            ErrorClass::ServerBusy { .. } => true,
            ErrorClass::HttpError(code) => matches!(code, 408 | 425 | 429 | 500 | 502 | 503 | 504),
        }
    }
}

/// Classify a transport-level (pre-response) failure.
pub fn classify_transport_error(err: &reqwest::Error) -> ErrorClass {
    if err.is_timeout() {
        return ErrorClass::Timeout;
    }
    if err.is_connect() {
        let msg = format!("{err:#}").to_lowercase();
        if msg.contains("dns")
            || msg.contains("name or service not known")
            || msg.contains("lookup")
            || msg.contains("nodename")
        {
            return ErrorClass::DnsFailure;
        }
        if msg.contains("tls")
            || msg.contains("ssl")
            || msg.contains("certificate")
            || msg.contains("handshake")
        {
            return ErrorClass::TlsFailure;
        }
    }
    if let Some(status) = err.status() {
        return classify_status(status.as_u16(), None);
    }
    ErrorClass::Other
}

/// Classify a non-success HTTP status. `retry_after` should be the parsed
/// `Retry-After` header (seconds form), if present.
pub fn classify_status(status_code: u16, retry_after: Option<Duration>) -> ErrorClass {
    match status_code {
        429 | 503 => ErrorClass::ServerBusy { retry_after },
        other => ErrorClass::HttpError(other),
    }
}

/// Parse a `Retry-After` header value. Supports the delay-seconds form;
/// the HTTP-date form is treated as "no hint" (caller falls back to
/// exponential backoff) since it requires wall-clock parsing we don't need
/// for Sprint 3's scope.
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_server_busy_with_retry_after() {
        let class = classify_status(429, Some(Duration::from_secs(5)));
        assert_eq!(
            class,
            ErrorClass::ServerBusy {
                retry_after: Some(Duration::from_secs(5))
            }
        );
        assert!(class.is_retryable());
    }

    #[test]
    fn classifies_503_as_server_busy() {
        assert_eq!(
            classify_status(503, None),
            ErrorClass::ServerBusy { retry_after: None }
        );
    }

    #[test]
    fn classifies_404_as_non_retryable_http_error() {
        let class = classify_status(404, None);
        assert_eq!(class, ErrorClass::HttpError(404));
        assert!(!class.is_retryable());
    }

    #[test]
    fn classifies_500_as_retryable_http_error() {
        let class = classify_status(500, None);
        assert!(class.is_retryable());
    }

    #[test]
    fn parses_retry_after_seconds() {
        assert_eq!(parse_retry_after("120"), Some(Duration::from_secs(120)));
        assert_eq!(parse_retry_after("not-a-number"), None);
    }
}
