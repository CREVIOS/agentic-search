//! Span: the unit of result returned to agents.
//!
//! A `Span` is always tied to a concrete byte range inside one object. AST
//! enrichment (in `as-ast`) widens a Line span into a Function / Class /
//! Method span; otherwise we return Line spans aligned to ripgrep matches.
//!
//! Optional metadata fields (`rank_signals`, `source_stage`,
//! `content_hash`, `truncated`) carry provenance the planner uses to
//! debug, log, and compress. Every optional field is
//! `skip_serializing_if = Option::is_none` so existing MCP / REST / SDK
//! clients that don't know about them keep parsing the JSON.

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

/// Which planner stage produced this span. Used by the planner for
/// debugging and by clients that want to weight stages differently.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceStage {
    /// Parallel ripgrep over the prefix.
    Grep,
    /// Tree-sitter span widening fed by a grep match.
    Ast,
    /// Centroid vector ANN.
    Vector,
    /// Fused result from `as-plan::rrf`.
    Fusion,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Span {
    pub uri: String,
    pub byte_range: Range<u64>,
    pub line_range: [u32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    pub kind: SpanKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    pub score: f32,

    // --- Optional metadata (added in v3.1). All fields default to None /
    //     empty so older serialised spans keep deserialising and older
    //     clients keep parsing the new shape. ---
    /// Per-signal scores (e.g. `{ "cosine": 0.74, "literal_match": 1.0 }`)
    /// gathered before fusion. Useful for explainability + reranking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank_signals: Option<RankSignals>,
    /// Which stage produced (or last touched) this span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_stage: Option<SourceStage>,
    /// SHA-256 (or other) of the underlying file content when the span
    /// was produced. Lets cache layers / clients detect drift between
    /// span issuance and span use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    /// `true` if `snippet` was truncated to keep response tokens bounded.
    #[serde(default, skip_serializing_if = "is_false")]
    pub truncated: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RankSignals {
    /// Cosine similarity vs. the query embedding when the vector stage
    /// produced this span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cosine: Option<f32>,
    /// 1.0 if a literal token in the query matched the snippet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub literal_match: Option<f32>,
    /// Number of distinct query terms found in the file. Lets the
    /// planner promote spans with broad term overlap on long files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub term_overlap: Option<u16>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_span_json_still_parses() {
        // Shape from before the v3.1 metadata fields. Must round-trip
        // without errors and the new optional fields stay None.
        let blob = serde_json::json!({
            "uri": "doc.md",
            "byte_range": {"start": 0, "end": 10},
            "line_range": [1, 1],
            "symbol": null,
            "kind": "line",
            "snippet": "hello",
            "score": 1.0
        });
        let s: Span = serde_json::from_value(blob).unwrap();
        assert_eq!(s.uri, "doc.md");
        assert_eq!(s.line_range, [1, 1]);
        assert!(s.rank_signals.is_none());
        assert!(s.source_stage.is_none());
        assert!(s.content_hash.is_none());
        assert!(!s.truncated);
    }

    #[test]
    fn span_with_metadata_serialises_compactly() {
        let s = Span {
            uri: "doc.md".into(),
            byte_range: 0..10,
            line_range: [1, 1],
            symbol: None,
            kind: SpanKind::Line,
            snippet: Some("hello".into()),
            score: 1.0,
            rank_signals: Some(RankSignals {
                cosine: Some(0.74),
                literal_match: Some(1.0),
                ..RankSignals::default()
            }),
            source_stage: Some(SourceStage::Fusion),
            content_hash: Some("sha256:abc".into()),
            truncated: true,
        };
        let v = serde_json::to_value(&s).unwrap();
        // The truncated=true field must be present; unset rank signals
        // are omitted because they were None.
        assert_eq!(v["truncated"], serde_json::json!(true));
        let cosine = v["rank_signals"]["cosine"].as_f64().unwrap();
        assert!((cosine - 0.74).abs() < 1e-3, "got cosine={cosine}");
        assert!(v["rank_signals"].get("term_overlap").is_none());
    }
}
