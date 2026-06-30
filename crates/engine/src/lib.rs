//! sdm-engine: the headless download orchestrator.
//!
//! Owns the Job/Segment model, the dynamic segment allocator, and the retry
//! state machine. UI-agnostic by design — see docs/ARCHITECTURE.md.
//!
//! Sprint 1 scope: single-segment HTTP/HTTPS download with progress events.

pub mod job;

pub use job::{Job, JobStatus};
