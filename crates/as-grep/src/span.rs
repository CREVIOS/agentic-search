//! Span: the unit of result returned to agents.
//!
//! A `Span` is always tied to a concrete byte range inside one object. AST
//! enrichment (in `as-ast`) widens a Line span into a Function / Class /
//! Method span; otherwise we return Line spans aligned to ripgrep matches.

use serde::{Deserialize, Serialize};
use std::ops::Range;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    #[default]
    Line,
    Block,
    Function,
    Method,
    Class,
    Module,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Span {
    pub uri: String,
    pub byte_range: Range<u64>,
    pub line_range: [u32; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    pub kind: SpanKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    pub score: f32,
}

impl Span {
    /// Unique key used to deduplicate spans across parallel search stages.
    pub fn dedup_key(&self) -> String {
        format!(
            "{}:{}-{}",
            self.uri, self.byte_range.start, self.byte_range.end
        )
    }
}
