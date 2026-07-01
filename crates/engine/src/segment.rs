//! Segment planning and the segment-stealing allocator.
//!
//! Sprint 2 scope: split a file into 1–128 byte-range segments and let an
//! idle worker "steal" the back half of the slowest still-in-progress
//! segment once its own segment is done, instead of sitting idle while one
//! straggler connection finishes alone.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::Mutex as AsyncMutex;

/// How many connections to use for a download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionsOption {
    Auto,
    Fixed(u32),
}

impl ConnectionsOption {
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        if s.eq_ignore_ascii_case("auto") {
            return Ok(ConnectionsOption::Auto);
        }
        let n: u32 = s
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid --connections value: {s}"))?;
        Ok(ConnectionsOption::Fixed(n))
    }
}

/// Bytes per connection for the "auto" heuristic — one connection per 2MB
/// of file, clamped to a sane default ceiling (well under the 128 hard cap)
/// so "auto" stays polite to small/typical servers by default.
const AUTO_BYTES_PER_CONNECTION: u64 = 2 * 1024 * 1024;
const AUTO_MAX_CONNECTIONS: u32 = 16;
pub const MAX_CONNECTIONS: u32 = 128;

/// Decide how many connections to use given probe results and the user's
/// request. Falls back to 1 if the server doesn't support ranges or we
/// don't know the file size.
pub fn choose_connection_count(
    total_bytes: Option<u64>,
    supports_range: bool,
    requested: ConnectionsOption,
) -> u32 {
    if !supports_range || total_bytes.is_none() {
        return 1;
    }
    let total = total_bytes.unwrap();
    let n = match requested {
        ConnectionsOption::Auto => {
            let n = (total / AUTO_BYTES_PER_CONNECTION).max(1);
            n.min(AUTO_MAX_CONNECTIONS as u64) as u32
        }
        ConnectionsOption::Fixed(n) => n,
    };
    n.clamp(1, MAX_CONNECTIONS)
}

/// Split `[0, total_bytes)` into `n` contiguous, roughly-equal, inclusive
/// byte ranges. Adaptive in the sense that the last `total_bytes % n`
/// segments are one byte larger, rather than leaving a tiny remainder
/// segment at the end.
pub fn plan_segments(total_bytes: u64, n: u32) -> Vec<(u64, u64)> {
    if total_bytes == 0 {
        return vec![(0, 0)];
    }
    let n = (n as u64).max(1).min(total_bytes.max(1));
    let base = total_bytes / n;
    let rem = total_bytes % n;
    let mut segments = Vec::with_capacity(n as usize);
    let mut start = 0u64;
    for i in 0..n {
        let size = base + u64::from(i < rem);
        if size == 0 {
            continue;
        }
        let end = start + size - 1;
        segments.push((start, end));
        start = end + 1;
    }
    segments
}

/// Live state for one in-flight (or stolen-into-existence) segment. Cheap
/// to clone — the heavy fields are `Arc`s shared with the worker task
/// that's actively downloading this range.
#[derive(Clone)]
pub struct SegmentRuntime {
    pub record_id: String,
    pub seq: i64,
    pub start: u64,
    /// Inclusive end byte. Shrinkable by the allocator (segment stealing).
    pub end: Arc<AtomicU64>,
    /// Next byte offset to be written. Updated by the downloading worker.
    pub position: Arc<AtomicU64>,
    pub done: Arc<AtomicBool>,
}

impl SegmentRuntime {
    pub fn new(record_id: String, seq: i64, start: u64, end: u64) -> Self {
        SegmentRuntime {
            record_id,
            seq,
            start,
            end: Arc::new(AtomicU64::new(end)),
            position: Arc::new(AtomicU64::new(start)),
            done: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn remaining(&self) -> u64 {
        let end = self.end.load(Ordering::SeqCst);
        let pos = self.position.load(Ordering::SeqCst);
        if pos > end {
            0
        } else {
            end - pos + 1
        }
    }
}

/// Minimum slice worth stealing — stealing a handful of bytes just adds
/// connection overhead for no real throughput gain.
const MIN_STEAL_BYTES: u64 = 64 * 1024;

/// Shared registry of all segments (completed, active, and
/// stolen-into-existence) for one job. Workers consult this when they
/// finish their own range and want more work.
pub struct SegmentAllocator {
    pub segments: AsyncMutex<Vec<SegmentRuntime>>,
    pub next_seq: AtomicU64,
}

impl SegmentAllocator {
    pub fn new(initial: Vec<SegmentRuntime>) -> Self {
        let next_seq = initial
            .iter()
            .map(|s| s.seq)
            .max()
            .map(|m| m + 1)
            .unwrap_or(0);
        SegmentAllocator {
            segments: AsyncMutex::new(initial),
            next_seq: AtomicU64::new(next_seq as u64),
        }
    }

    /// Try to steal the back half of whichever active segment has the most
    /// remaining work. Returns the newly created runtime (already inserted
    /// into the registry) if a steal happened.
    pub async fn try_steal(&self) -> Option<SegmentRuntime> {
        let mut guard = self.segments.lock().await;

        let mut best_idx: Option<usize> = None;
        let mut best_remaining: u64 = MIN_STEAL_BYTES * 2; // require enough to make two halves worth it

        for (i, seg) in guard.iter().enumerate() {
            if seg.done.load(Ordering::SeqCst) {
                continue;
            }
            let pos = seg.position.load(Ordering::SeqCst);
            let end = seg.end.load(Ordering::SeqCst);
            if end < pos {
                continue;
            }
            let remaining = end - pos + 1;
            if remaining > best_remaining {
                best_remaining = remaining;
                best_idx = Some(i);
            }
        }

        let idx = best_idx?;
        let (pos, end) = {
            let target = &guard[idx];
            (
                target.position.load(Ordering::SeqCst),
                target.end.load(Ordering::SeqCst),
            )
        };

        if end - pos + 1 < MIN_STEAL_BYTES * 2 {
            return None;
        }

        let mid = pos + (end - pos) / 2;
        guard[idx].end.store(mid, Ordering::SeqCst);

        let new_seq = self.next_seq.fetch_add(1, Ordering::SeqCst) as i64;
        let new_runtime =
            SegmentRuntime::new(format!("seg-steal-{new_seq}"), new_seq, mid + 1, end);
        guard.push(new_runtime.clone());
        Some(new_runtime)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_count_falls_back_to_one_without_range_support() {
        assert_eq!(
            choose_connection_count(Some(100_000_000), false, ConnectionsOption::Fixed(8)),
            1
        );
        assert_eq!(
            choose_connection_count(None, true, ConnectionsOption::Fixed(8)),
            1
        );
    }

    #[test]
    fn connection_count_respects_fixed_request_within_bounds() {
        assert_eq!(
            choose_connection_count(Some(100_000_000), true, ConnectionsOption::Fixed(8)),
            8
        );
        // Clamped to the hard cap even if the user asks for more.
        assert_eq!(
            choose_connection_count(Some(100_000_000), true, ConnectionsOption::Fixed(999)),
            128
        );
        assert_eq!(
            choose_connection_count(Some(100_000_000), true, ConnectionsOption::Fixed(0)),
            1
        );
    }

    #[test]
    fn connection_count_auto_scales_with_size() {
        let small = choose_connection_count(Some(1024), true, ConnectionsOption::Auto);
        let large = choose_connection_count(Some(500 * 1024 * 1024), true, ConnectionsOption::Auto);
        assert_eq!(small, 1);
        assert!(large > small);
        assert!(large <= AUTO_MAX_CONNECTIONS);
    }

    #[test]
    fn plan_segments_covers_whole_file_without_gaps_or_overlap() {
        for (total, n) in [(1000u64, 4u32), (999, 4), (1, 8), (7, 3), (1_000_000, 8)] {
            let segs = plan_segments(total, n);
            assert_eq!(segs[0].0, 0);
            assert_eq!(segs.last().unwrap().1, total - 1);
            for w in segs.windows(2) {
                assert_eq!(
                    w[0].1 + 1,
                    w[1].0,
                    "segments must be contiguous with no gap/overlap"
                );
            }
            let covered: u64 = segs.iter().map(|(s, e)| e - s + 1).sum();
            assert_eq!(covered, total);
        }
    }

    #[tokio::test]
    async fn steal_splits_the_largest_remaining_segment() {
        let seg_a = SegmentRuntime::new("a".into(), 0, 0, 999_999); // huge, untouched
        let seg_b = SegmentRuntime::new("b".into(), 1, 1_000_000, 1_000_999); // tiny, done
        seg_b.done.store(true, Ordering::SeqCst);

        let allocator = SegmentAllocator::new(vec![seg_a.clone(), seg_b]);
        let stolen = allocator
            .try_steal()
            .await
            .expect("should steal from seg_a");
        let shrunk_end = seg_a.end.load(Ordering::SeqCst);

        assert_eq!(stolen.start, shrunk_end + 1);
        assert_eq!(stolen.end.load(Ordering::SeqCst), 999_999);
        // seg_a's end must have shrunk to give away the back half.
        assert!(shrunk_end < 999_999);

        let guard = allocator.segments.lock().await;
        assert_eq!(guard.len(), 3);
    }

    #[tokio::test]
    async fn no_steal_when_remaining_work_is_too_small() {
        let seg_a = SegmentRuntime::new("a".into(), 0, 0, 1000); // small segment
        let allocator = SegmentAllocator::new(vec![seg_a]);
        assert!(allocator.try_steal().await.is_none());
    }

    #[tokio::test]
    async fn no_steal_when_all_segments_done() {
        let seg_a = SegmentRuntime::new("a".into(), 0, 0, 999_999);
        seg_a.done.store(true, Ordering::SeqCst);
        let allocator = SegmentAllocator::new(vec![seg_a]);
        assert!(allocator.try_steal().await.is_none());
    }
}
