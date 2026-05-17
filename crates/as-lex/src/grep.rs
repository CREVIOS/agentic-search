//! ripgrep-as-library scan over a byte buffer fetched from the store.
//!
//! Caller streams bytes (full object or range) and we run `grep-searcher`
//! against them. The intent is to keep ripgrep semantics (regex flavor,
//! line-by-line, multi-line) without spawning a subprocess.

use as_core::{Error, Hit, Result};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::{sinks::UTF8, Searcher};

#[derive(Clone, Debug, Default)]
pub struct GrepOpts {
    pub case_insensitive: bool,
    pub multi_line: bool,
    pub max_hits_per_file: Option<usize>,
}

/// Run a regex search over a single byte buffer associated with `uri`.
pub fn grep_bytes(uri: &str, bytes: &[u8], pattern: &str, opts: &GrepOpts) -> Result<Vec<Hit>> {
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(opts.case_insensitive)
        .multi_line(opts.multi_line)
        .build(pattern)
        .map_err(|e| Error::Index(format!("bad regex: {e}")))?;

    let mut hits: Vec<Hit> = Vec::new();
    let cap = opts.max_hits_per_file.unwrap_or(usize::MAX);

    Searcher::new()
        .search_slice(
            &matcher,
            bytes,
            UTF8(|lnum, line| {
                if hits.len() >= cap {
                    return Ok(false);
                }
                hits.push(Hit {
                    id: format!("{uri}:{lnum}"),
                    uri: uri.to_string(),
                    score: 1.0,
                    snippet: Some(line.trim_end().to_string()),
                    metadata: serde_json::json!({ "line": lnum }),
                });
                Ok(true)
            }),
        )
        .map_err(|e| Error::Index(format!("grep: {e}")))?;

    Ok(hits)
}
