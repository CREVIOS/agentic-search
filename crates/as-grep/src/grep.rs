//! ripgrep-as-library scan over a byte buffer fetched from the store.
//!
//! Caller streams bytes (full object or range) and we run `grep-searcher`
//! against them. The intent is to keep ripgrep semantics (regex flavor,
//! line-by-line, multi-line) without spawning a subprocess.

use crate::{SourceStage, Span, SpanKind};
use as_core::{Error, Hit, Result};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{Searcher, Sink, SinkMatch};
use std::io;

#[derive(Clone, Debug, Default)]
pub struct GrepOpts {
    pub case_insensitive: bool,
    pub multi_line: bool,
    pub max_hits_per_file: Option<usize>,
}

/// Run a regex search over a single byte buffer associated with `uri`,
/// returning legacy `Hit` shape (still used by the old CLI smoke path).
pub fn grep_bytes(uri: &str, bytes: &[u8], pattern: &str, opts: &GrepOpts) -> Result<Vec<Hit>> {
    let spans = grep_bytes_spans(uri, bytes, pattern, opts)?;
    Ok(spans
        .into_iter()
        .map(|s| Hit {
            id: format!("{}:{}", s.uri, s.line_range[0]),
            uri: s.uri,
            score: s.score,
            snippet: s.snippet,
            metadata: serde_json::json!({ "line": s.line_range[0] }),
        })
        .collect())
}

/// Run a regex search over a single byte buffer and return `Span`s.
///
/// Each span is one matching line, anchored to a precise byte range. The
/// caller (planner / AST enricher) may later expand a `Line` span into a
/// `Function` / `Class` / `Method` span using tree-sitter.
pub fn grep_bytes_spans(
    uri: &str,
    bytes: &[u8],
    pattern: &str,
    opts: &GrepOpts,
) -> Result<Vec<Span>> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(opts.case_insensitive)
        .multi_line(opts.multi_line)
        .build(pattern)
        .map_err(|e| Error::Index(format!("bad regex: {e}")))?;

    let cap = opts.max_hits_per_file.unwrap_or(usize::MAX);
    let mut sink = SpanSink {
        uri,
        spans: Vec::new(),
        cap,
    };

    Searcher::new()
        .search_slice(&matcher, bytes, &mut sink)
        .map_err(|e| Error::Index(format!("grep: {e}")))?;

    Ok(sink.spans)
}

struct SpanSink<'a> {
    uri: &'a str,
    spans: Vec<Span>,
    cap: usize,
}

impl Sink for SpanSink<'_> {
    type Error = io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> io::Result<bool> {
        if self.spans.len() >= self.cap {
            return Ok(false);
        }
        let line_start = mat.line_number().unwrap_or(1) as u32;
        let line_count = mat.lines().count().max(1) as u32;
        let line_end = line_start.saturating_add(line_count.saturating_sub(1));
        let start = mat.absolute_byte_offset();
        let end = start.saturating_add(mat.bytes().len() as u64);
        let snippet = String::from_utf8_lossy(mat.bytes()).trim_end().to_string();
        self.spans.push(Span {
            uri: self.uri.to_string(),
            byte_range: start..end,
            line_range: [line_start, line_end],
            kind: SpanKind::Line,
            snippet: Some(snippet),
            score: 1.0,
            source_stage: Some(SourceStage::Grep),
            ..Span::default()
        });
        Ok(self.spans.len() < self.cap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_have_correct_byte_ranges() {
        let bytes = b"alpha\nbravo TODO foo\ncharlie\nbravo TODO bar\n";
        let spans = grep_bytes_spans("test://x", bytes, "TODO", &GrepOpts::default()).unwrap();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].line_range, [2, 2]);
        let s0_text = &bytes[spans[0].byte_range.start as usize..spans[0].byte_range.end as usize];
        assert_eq!(s0_text, b"bravo TODO foo\n");
        assert_eq!(spans[1].line_range, [4, 4]);
        let s1_text = &bytes[spans[1].byte_range.start as usize..spans[1].byte_range.end as usize];
        assert_eq!(s1_text, b"bravo TODO bar\n");
    }
}
