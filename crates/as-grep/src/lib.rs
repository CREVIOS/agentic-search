//! Ripgrep-as-library: scan a single buffer or a whole object-store prefix.
//!
//! No subprocess. `grep-searcher` is linked in, so we keep ripgrep's regex
//! flavor and line semantics without paying for fork/exec on every call.
//! The prefix scanner fans reads out in parallel against the underlying
//! filesystem, deduplicates hits per file, and yields `Span`s.

pub mod grep;
pub mod parallel;
pub mod span;

pub use grep::{grep_bytes, grep_bytes_spans, GrepOpts};
pub use parallel::{ParallelGrep, ParallelOpts};
pub use span::{RankSignals, SourceStage, Span, SpanKind};
