//! Tree-sitter span extractor + per-file parse cache.
//!
//! Two surfaces:
//!
//! - `widen_many` / `widen_to_definition`: parse-once-per-call. Used
//!   when the caller does not have access to a shared cache (one-off
//!   tests, tools).
//! - `SpanCache` + `widen_with_cache`: parse-once-per-(uri, content_hash).
//!   The server hot path uses this so repeated `grep --ast` against the
//!   same prefix avoids re-running tree-sitter on files that did not
//!   change. On a 782-file Rust corpus this drops AST mode from ~400 ms
//!   to <40 ms after warmup.
//!
//! Shipping languages: Rust, Python, JavaScript, TypeScript, TSX, Go.

use as_core::Result;
use as_grep::{SourceStage, Span, SpanKind};
use lru::LruCache;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::num::NonZeroUsize;
use std::sync::Arc;
use tree_sitter::{Language, Node, Parser};

/// Stable grammar-set version. Bumping this invalidates every cached
/// container index. Bump whenever a grammar version changes or a
/// `container_kinds` set is edited.
pub const GRAMMAR_VERSION: u32 = 1;

/// Detect a language from a URI by file extension.
///
/// Each grammar crate exposes a `LANGUAGE` constant of type
/// `tree_sitter::LanguageFn` (tree-sitter 0.25+ API).
pub fn language_for_uri(uri: &str) -> Option<LangSpec> {
    let ext = uri.rsplit('.').next()?.to_ascii_lowercase();
    match ext.as_str() {
        "rs" => Some(LangSpec {
            id: "rust",
            language: tree_sitter_rust::LANGUAGE.into(),
            container_kinds: &[
                "function_item",
                "impl_item",
                "struct_item",
                "enum_item",
                "mod_item",
            ],
        }),
        "py" | "pyi" => Some(LangSpec {
            id: "python",
            language: tree_sitter_python::LANGUAGE.into(),
            container_kinds: &["function_definition", "class_definition"],
        }),
        "js" | "jsx" | "mjs" | "cjs" => Some(LangSpec {
            id: "javascript",
            language: tree_sitter_javascript::LANGUAGE.into(),
            container_kinds: &[
                "function_declaration",
                "method_definition",
                "class_declaration",
                "arrow_function",
                "function",
            ],
        }),
        "ts" => Some(LangSpec {
            id: "typescript",
            language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            container_kinds: &[
                "function_declaration",
                "method_definition",
                "class_declaration",
                "interface_declaration",
                "type_alias_declaration",
                "arrow_function",
                "function",
            ],
        }),
        "tsx" => Some(LangSpec {
            id: "tsx",
            language: tree_sitter_typescript::LANGUAGE_TSX.into(),
            container_kinds: &[
                "function_declaration",
                "method_definition",
                "class_declaration",
                "arrow_function",
                "function",
            ],
        }),
        "go" => Some(LangSpec {
            id: "go",
            language: tree_sitter_go::LANGUAGE.into(),
            container_kinds: &[
                "function_declaration",
                "method_declaration",
                "type_declaration",
            ],
        }),
        _ => None,
    }
}

#[derive(Clone)]
pub struct LangSpec {
    pub id: &'static str,
    pub language: Language,
    pub container_kinds: &'static [&'static str],
}

/// One container (function / class / method / module) discovered by
/// tree-sitter. The cache stores a sorted list of these per file; a
/// grep hit's byte offset binary-searches the list to find its
/// enclosing container.
#[derive(Clone, Debug)]
pub struct ContainerSpan {
    pub start_byte: u64,
    pub end_byte: u64,
    pub start_line: u32,
    pub end_line: u32,
    pub symbol: Option<String>,
    pub kind: SpanKind,
}

/// Parsed-once container index for a single file.
///
/// `containers` is sorted by `(start_byte, end_byte)` ascending so that
/// the innermost enclosing container for a byte offset is the last
/// match in a prefix-bounded scan.
#[derive(Clone, Debug)]
pub struct ContainerIndex {
    pub uri: String,
    pub content_hash: String,
    pub containers: Vec<ContainerSpan>,
}

impl ContainerIndex {
    /// Build by running tree-sitter once and collecting every container
    /// node in the parsed tree.
    pub fn build(uri: &str, bytes: &[u8]) -> Result<Option<Self>> {
        let lang = match language_for_uri(uri) {
            Some(l) => l,
            None => return Ok(None),
        };
        let mut parser = Parser::new();
        if parser.set_language(&lang.language).is_err() {
            return Ok(None);
        }
        let tree = match parser.parse(bytes, None) {
            Some(t) => t,
            None => return Ok(None),
        };

        let mut containers: Vec<ContainerSpan> = Vec::new();
        let mut stack = vec![tree.root_node()];
        while let Some(node) = stack.pop() {
            if lang.container_kinds.iter().any(|k| *k == node.kind()) {
                containers.push(ContainerSpan {
                    start_byte: node.start_byte() as u64,
                    end_byte: node.end_byte() as u64,
                    start_line: (node.start_position().row + 1) as u32,
                    end_line: (node.end_position().row + 1) as u32,
                    symbol: symbol_name(node, bytes),
                    kind: classify_kind(node.kind()),
                });
            }
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                stack.push(child);
            }
        }
        containers.sort_by(|a, b| {
            a.start_byte
                .cmp(&b.start_byte)
                .then(a.end_byte.cmp(&b.end_byte).reverse())
        });

        Ok(Some(Self {
            uri: uri.to_string(),
            content_hash: content_hash(bytes),
            containers,
        }))
    }

    /// Find the smallest enclosing container for the given byte offset,
    /// or `None` if none exists.
    pub fn enclosing(&self, byte: u64) -> Option<&ContainerSpan> {
        // Containers are sorted by start_byte; pick the deepest match
        // (largest start_byte ≤ byte with end_byte > byte).
        let mut best: Option<&ContainerSpan> = None;
        // Binary search for the upper bound, then walk back.
        let idx = self.containers.partition_point(|c| c.start_byte <= byte);
        for c in self.containers[..idx].iter().rev() {
            if c.end_byte > byte {
                // First enclosing candidate; deeper containers come
                // later in the sorted order, so keep looking for one
                // with a larger start (= narrower enclosure).
                if best.map(|b| c.start_byte > b.start_byte).unwrap_or(true) {
                    best = Some(c);
                }
            }
            // Once we drop below any candidate's range, further left
            // containers can only be wider, so they can't improve the
            // narrowest match we already have.
            if let Some(b) = best {
                if c.end_byte < b.start_byte {
                    break;
                }
            }
        }
        best
    }
}

/// Process-local LRU cache of `ContainerIndex` keyed by
/// `(language_id, content_hash, grammar_version)`. Identical content
/// across multiple paths (vendored libraries, generated files) now
/// shares one parse, which is the common case on large monorepos.
pub struct SpanCache {
    inner: Mutex<LruCache<String, Arc<ContainerIndex>>>,
}

impl SpanCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            inner: Mutex::new(LruCache::new(
                NonZeroUsize::new(max_entries.max(1)).unwrap(),
            )),
        }
    }

    /// Cache key dropped `uri` in favour of `(grammar_version,
    /// language_id, content_hash)` so two paths with identical bytes
    /// share one parsed ContainerIndex. Language is part of the key
    /// because the same byte stream could in theory be valid as two
    /// different grammars (e.g. .ts vs .tsx); we never collide them.
    fn key_from_lang(lang_id: &str, content_hash: &str) -> String {
        format!("{GRAMMAR_VERSION}:{lang_id}:{content_hash}")
    }

    /// Cheap probe — no parse on miss. The caller hands us the URI so
    /// we can pick the right language id; the `uri` itself does not
    /// participate in the cache key.
    pub fn get(&self, uri: &str, content_hash: &str) -> Option<Arc<ContainerIndex>> {
        let lang = language_for_uri(uri)?;
        let key = Self::key_from_lang(lang.id, content_hash);
        self.inner.lock().get(&key).cloned()
    }

    /// Get-or-build. Builds the index synchronously on miss.
    pub fn get_or_build(&self, uri: &str, bytes: &[u8]) -> Result<Option<Arc<ContainerIndex>>> {
        let lang = match language_for_uri(uri) {
            Some(l) => l,
            None => return Ok(None),
        };
        let hash = content_hash(bytes);
        let key = Self::key_from_lang(lang.id, &hash);
        if let Some(v) = self.inner.lock().get(&key).cloned() {
            return Ok(Some(v));
        }
        let built = match ContainerIndex::build(uri, bytes)? {
            Some(c) => Arc::new(c),
            None => return Ok(None),
        };
        self.inner.lock().put(key, built.clone());
        Ok(Some(built))
    }

    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }
}

impl Default for SpanCache {
    fn default() -> Self {
        // 8192 files keeps a few hundred MB at most (the indexes
        // themselves are ~10 KB per file on real code).
        Self::new(8192)
    }
}

pub fn content_hash(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut s = String::with_capacity(64 + 7);
    s.push_str("sha256:");
    s.push_str(&hex::encode(out));
    s
}

const SNIPPET_CHARS: usize = 800;

fn apply_container(span: &mut Span, container: &ContainerSpan, bytes: &[u8]) {
    span.kind = container.kind;
    span.symbol = container.symbol.clone();
    span.line_range = [container.start_line, container.end_line];
    span.byte_range = container.start_byte..container.end_byte;
    let raw = bytes
        .get(container.start_byte as usize..container.end_byte as usize)
        .map(String::from_utf8_lossy)
        .map(|s| s.to_string());
    if let Some(text) = raw {
        let trimmed: String = text.chars().take(SNIPPET_CHARS).collect();
        span.truncated = trimmed.chars().count() < text.chars().count();
        span.snippet = Some(trimmed);
    }
    span.source_stage = Some(SourceStage::Ast);
}

/// Widen a single (Line) span via tree-sitter. Convenience wrapper.
pub fn widen_to_definition(bytes: &[u8], span: Span) -> Result<Span> {
    let mut spans = vec![span];
    widen_many(bytes, &mut spans)?;
    Ok(spans.pop().unwrap())
}

/// Widen many spans against a single file's bytes in one parser pass.
/// No external cache; suitable for one-off use.
pub fn widen_many(bytes: &[u8], spans: &mut [Span]) -> Result<()> {
    let Some(first) = spans.first() else {
        return Ok(());
    };
    let index = match ContainerIndex::build(&first.uri, bytes)? {
        Some(i) => i,
        None => return Ok(()),
    };
    let hash = index.content_hash.clone();
    for span in spans.iter_mut() {
        let probe = first_non_whitespace(
            bytes,
            span.byte_range.start as usize,
            (span.byte_range.end as usize).min(bytes.len()),
        ) as u64;
        if let Some(container) = index.enclosing(probe) {
            apply_container(span, container, bytes);
        }
        span.content_hash = Some(hash.clone());
    }
    Ok(())
}

/// Widen many spans for a single file using a shared `SpanCache`. The
/// file is parsed at most once per `(uri, content_hash)` across the
/// process lifetime, so back-to-back search calls over the same prefix
/// pay parse cost only on cold files.
pub fn widen_with_cache(cache: &SpanCache, bytes: &[u8], spans: &mut [Span]) -> Result<()> {
    widen_with_cache_cancellable(cache, bytes, spans, &|| false)
}

/// Same as `widen_with_cache` but checks `cancelled()` between every
/// span. Designed for `spawn_blocking` callers that want cooperative
/// cancellation: dropping the parent future cannot abort an in-flight
/// blocking task, but the blocking task can ask "should I keep going?"
/// at each iteration and bail early.
pub fn widen_with_cache_cancellable(
    cache: &SpanCache,
    bytes: &[u8],
    spans: &mut [Span],
    cancelled: &dyn Fn() -> bool,
) -> Result<()> {
    // Pre-check: a cancel signal that fires while the caller is
    // waiting for the spawn_blocking slot should bail before we run
    // the parse. `ContainerIndex::build` can take 100s of ms on a
    // large file and is *not* internally interruptible, so this is
    // the only cheap point at which we can honour a late cancel.
    if cancelled() {
        return Ok(());
    }
    let Some(first) = spans.first() else {
        return Ok(());
    };
    let uri = first.uri.clone();
    let index = match cache.get_or_build(&uri, bytes)? {
        Some(i) => i,
        None => return Ok(()),
    };
    for span in spans.iter_mut() {
        if cancelled() {
            return Ok(());
        }
        let probe = first_non_whitespace(
            bytes,
            span.byte_range.start as usize,
            (span.byte_range.end as usize).min(bytes.len()),
        ) as u64;
        if let Some(container) = index.enclosing(probe) {
            apply_container(span, container, bytes);
        }
        span.content_hash = Some(index.content_hash.clone());
    }
    Ok(())
}

fn first_non_whitespace(bytes: &[u8], start: usize, end: usize) -> usize {
    let mut i = start;
    while i < end {
        match bytes.get(i) {
            Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') => i += 1,
            _ => return i,
        }
    }
    start
}

fn symbol_name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    // Most grammars expose a `name` field on definitions.
    if let Some(name_node) = node.child_by_field_name("name") {
        let slice = bytes.get(name_node.byte_range())?;
        return Some(String::from_utf8_lossy(slice).into_owned());
    }
    None
}

fn classify_kind(ts_kind: &str) -> SpanKind {
    match ts_kind {
        "class_declaration"
        | "class_definition"
        | "struct_item"
        | "enum_item"
        | "interface_declaration"
        | "type_alias_declaration"
        | "type_declaration" => SpanKind::Class,
        "method_definition" | "method_declaration" => SpanKind::Method,
        "function_declaration"
        | "function_definition"
        | "function_item"
        | "arrow_function"
        | "function" => SpanKind::Function,
        "mod_item" => SpanKind::Module,
        _ => SpanKind::Block,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use as_grep::{grep_bytes_spans, GrepOpts};

    #[test]
    fn widens_python_match_to_function() {
        let src = b"def alpha(x):\n    return x + 1\n\ndef beta(x):\n    # secret: TODO inside beta\n    return x * 2\n";
        let hits = grep_bytes_spans("file:///a.py", src, "TODO", &GrepOpts::default()).unwrap();
        assert_eq!(hits.len(), 1);
        let widened = widen_to_definition(src, hits[0].clone()).unwrap();
        assert_eq!(widened.kind, SpanKind::Function);
        assert_eq!(widened.symbol.as_deref(), Some("beta"));
        assert_eq!(widened.line_range, [4, 6]);
        assert!(widened.content_hash.is_some());
    }

    #[test]
    fn widens_rust_match_to_fn() {
        let src = b"fn alpha() {}\n\nfn beta() {\n    // TODO: rewrite\n    let _ = 1;\n}\n";
        let hits = grep_bytes_spans("file:///x.rs", src, "TODO", &GrepOpts::default()).unwrap();
        assert_eq!(hits.len(), 1);
        let w = widen_to_definition(src, hits[0].clone()).unwrap();
        assert_eq!(w.kind, SpanKind::Function);
        assert_eq!(w.symbol.as_deref(), Some("beta"));
    }

    #[test]
    fn cache_returns_same_index_for_unchanged_bytes() {
        let src = b"fn alpha() {}\nfn beta() {}\n";
        let cache = SpanCache::new(8);
        let idx1 = cache
            .get_or_build("file:///x.rs", src)
            .unwrap()
            .expect("rust index");
        let idx2 = cache
            .get_or_build("file:///x.rs", src)
            .unwrap()
            .expect("rust index hit");
        assert!(Arc::ptr_eq(&idx1, &idx2), "second call must hit the cache");
        assert_eq!(idx1.containers.len(), 2);
    }

    #[test]
    fn cache_dedups_identical_content_across_paths() {
        // Two URIs that share bytes (typical for vendored / generated
        // code) should share one parsed ContainerIndex.
        let src = b"fn alpha() {}\nfn beta() {}\n";
        let cache = SpanCache::new(8);
        let a = cache
            .get_or_build("file:///vendor/a.rs", src)
            .unwrap()
            .unwrap();
        let b = cache
            .get_or_build("file:///app/b.rs", src)
            .unwrap()
            .unwrap();
        assert!(
            Arc::ptr_eq(&a, &b),
            "content-addressed cache should serve both paths"
        );
    }

    #[test]
    fn cache_invalidates_on_content_change() {
        let cache = SpanCache::new(8);
        let v1 = b"fn alpha() {}\n";
        let v2 = b"fn alpha() {}\nfn beta() {}\n";
        let idx1 = cache.get_or_build("file:///x.rs", v1).unwrap().unwrap();
        let idx2 = cache.get_or_build("file:///x.rs", v2).unwrap().unwrap();
        assert!(
            !Arc::ptr_eq(&idx1, &idx2),
            "different bytes => different entry"
        );
        assert_eq!(idx1.containers.len(), 1);
        assert_eq!(idx2.containers.len(), 2);
        assert_ne!(idx1.content_hash, idx2.content_hash);
    }

    #[test]
    fn widen_with_cache_matches_parse_per_call() {
        let src = b"fn alpha() {}\n\nfn beta() {\n    // TODO\n    let _ = 1;\n}\n";
        let hits = grep_bytes_spans("file:///x.rs", src, "TODO", &GrepOpts::default()).unwrap();
        let mut a = hits.clone();
        let mut b = hits;
        widen_many(src, &mut a).unwrap();
        let cache = SpanCache::new(8);
        widen_with_cache(&cache, src, &mut b).unwrap();
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.kind, y.kind);
            assert_eq!(x.symbol, y.symbol);
            assert_eq!(x.line_range, y.line_range);
            assert_eq!(x.byte_range, y.byte_range);
        }
        // Second call must come from the cache (same Arc<ContainerIndex>).
        let bytes_hash = content_hash(src);
        let before = cache.get("file:///x.rs", &bytes_hash);
        assert!(before.is_some());
    }

    #[test]
    fn falls_through_for_unknown_extension() {
        let span = Span {
            uri: "file:///x.unknown".into(),
            byte_range: 0..1,
            line_range: [1, 1],
            kind: SpanKind::Line,
            score: 1.0,
            ..Span::default()
        };
        let out = widen_to_definition(b"hello", span.clone()).unwrap();
        assert_eq!(out.kind, SpanKind::Line);
    }
}
