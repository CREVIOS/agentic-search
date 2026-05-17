//! Lexical search layer.
//!
//! - `grep`: ripgrep-as-library scan over an FS prefix (no subprocess).
//! - `index`: tantivy BM25 index with segments persisted to the object store.

use as_core::{Hit, Result};

pub mod grep;
pub mod tantivy_index;

/// Hits ranked by BM25-ish lexical score.
pub trait LexicalSearch {
    fn search(&self, query: &str, k: usize) -> Result<Vec<Hit>>;
}
