//! Mirror support (Sprint 4).
//!
//! A job can be given more than one URL for the same content. At probe
//! time we measure each mirror's round-trip latency (a cheap proxy for
//! "how fast is this server likely to be" — actually downloading a test
//! range from every mirror just to rank them would waste bandwidth) and
//! rank them fastest-first. Segment workers start on the fastest mirror
//! and, on a retryable failure, rotate to the next mirror rather than
//! hammering the same failing server — "auto-switch on failure, continue
//! partial download from a different mirror."

use std::time::Instant;

use reqwest::Client;

/// Result of probing one candidate mirror URL.
#[derive(Debug, Clone)]
pub struct MirrorProbe {
    pub url: String,
    /// `None` means the mirror was unreachable at probe time.
    pub latency_ms: Option<u64>,
}

/// Probe every URL with a lightweight `HEAD` request and record latency.
/// Order of the input is not preserved; the caller should use
/// [`rank_by_latency`] on the result to get a fastest-first ordering.
pub async fn probe_mirrors(client: &Client, urls: &[String]) -> Vec<MirrorProbe> {
    let mut out = Vec::with_capacity(urls.len());
    for url in urls {
        let start = Instant::now();
        let reachable = client.head(url).send().await.is_ok();
        let latency_ms = if reachable {
            Some(start.elapsed().as_millis() as u64)
        } else {
            None
        };
        out.push(MirrorProbe {
            url: url.clone(),
            latency_ms,
        });
    }
    out
}

/// Sort probes fastest-first; unreachable mirrors (`latency_ms: None`) sink
/// to the end but are kept (a mirror that's briefly unreachable at probe
/// time might still work once we actually try it).
pub fn rank_by_latency(mut probes: Vec<MirrorProbe>) -> Vec<MirrorProbe> {
    probes.sort_by_key(|p| p.latency_ms.unwrap_or(u64::MAX));
    probes
}

/// A ranked, rotatable set of mirror URLs for one job. Cheap to clone (just
/// `Arc`-wraps a `Vec<String>` conceptually via `Clone` on `Vec<String>` —
/// small lists, cloned once per worker task at spawn time).
#[derive(Debug, Clone)]
pub struct MirrorSet {
    urls: Vec<String>,
}

impl MirrorSet {
    /// `urls` must be non-empty and already ranked fastest-first.
    pub fn new(urls: Vec<String>) -> Self {
        assert!(!urls.is_empty(), "MirrorSet requires at least one URL");
        MirrorSet { urls }
    }

    pub fn primary(&self) -> &str {
        &self.urls[0]
    }

    pub fn urls(&self) -> &[String] {
        &self.urls
    }

    pub fn len(&self) -> usize {
        self.urls.len()
    }

    pub fn is_empty(&self) -> bool {
        false // invariant: constructor requires non-empty
    }

    /// Which URL a given (1-indexed) attempt number should use. Attempt 1
    /// always uses the fastest-ranked mirror; each subsequent attempt
    /// rotates to the next mirror in ranked order, wrapping around. This is
    /// the "auto-switch on failure" behavior — a retryable failure on the
    /// current mirror moves the *next* attempt to a different one.
    pub fn pick_for_attempt(&self, attempt: u32) -> &str {
        let idx = (attempt.saturating_sub(1)) as usize % self.urls.len();
        &self.urls[idx]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rank_by_latency_sorts_fastest_first_and_keeps_unreachable() {
        let probes = vec![
            MirrorProbe {
                url: "https://slow.example.com".into(),
                latency_ms: Some(500),
            },
            MirrorProbe {
                url: "https://fast.example.com".into(),
                latency_ms: Some(20),
            },
            MirrorProbe {
                url: "https://dead.example.com".into(),
                latency_ms: None,
            },
        ];
        let ranked = rank_by_latency(probes);
        assert_eq!(ranked[0].url, "https://fast.example.com");
        assert_eq!(ranked[1].url, "https://slow.example.com");
        assert_eq!(ranked[2].url, "https://dead.example.com");
    }

    #[test]
    fn mirror_set_rotates_on_successive_attempts() {
        let set = MirrorSet::new(vec![
            "https://a.example.com".into(),
            "https://b.example.com".into(),
            "https://c.example.com".into(),
        ]);
        assert_eq!(set.pick_for_attempt(1), "https://a.example.com");
        assert_eq!(set.pick_for_attempt(2), "https://b.example.com");
        assert_eq!(set.pick_for_attempt(3), "https://c.example.com");
        // Wraps back around.
        assert_eq!(set.pick_for_attempt(4), "https://a.example.com");
    }

    #[test]
    fn mirror_set_single_url_always_returns_it() {
        let set = MirrorSet::new(vec!["https://only.example.com".into()]);
        assert_eq!(set.pick_for_attempt(1), "https://only.example.com");
        assert_eq!(set.pick_for_attempt(7), "https://only.example.com");
    }
}
