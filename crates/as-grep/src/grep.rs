//! ripgrep-as-library scan over a byte buffer fetched from the store.
//!
//! Caller streams bytes (full object or range) and we run `grep-searcher`
//! against them. The intent is to keep ripgrep semantics (regex flavor,
//! line-by-line, multi-line) without spawning a subprocess.

use crate::{Span, SpanKind};
use as_core::{Error, Hit, Result};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{sinks::UTF8, Searcher};

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
    let mut spans: Vec<Span> = Vec::new();
    // Precompute line starts so we can recover byte ranges.
    let line_starts = line_offsets(bytes);

    Searcher::new()
        .search_slice(
            &matcher,
            bytes,
            UTF8(|lnum, line| {
                if spans.len() >= cap {
                    return Ok(false);
                }
                let idx = (lnum.saturating_sub(1)) as usize;
                let start = line_starts.get(idx).copied().unwrap_or(0);
                let end = line_starts
                    .get(idx + 1)
                    .copied()
                    .unwrap_or(bytes.len() as u64);
                spans.push(Span {
                    uri: uri.to_string(),
                    byte_range: start..end,
                    line_range: [lnum as u32, lnum as u32],
                    symbol: None,
                    kind: SpanKind::Line,
                    snippet: Some(line.trim_end().to_string()),
                    score: 1.0,
                });
                Ok(true)
            }),
        )
        .map_err(|e| Error::Index(format!("grep: {e}")))?;

    Ok(spans)
}

/// Byte offsets of the start of each line in `bytes`. `line_offsets(b)[i]`
/// is the start of the (i+1)-th line; `line_offsets(b).last()` equals
/// `bytes.len()`.
fn line_offsets(bytes: &[u8]) -> Vec<u64> {
    let mut offsets = Vec::with_capacity(64);
    offsets.push(0);
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            offsets.push((i + 1) as u64);
        }
    }
    if *offsets.last().unwrap() != bytes.len() as u64 {
        offsets.push(bytes.len() as u64);
    }
    offsets
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
