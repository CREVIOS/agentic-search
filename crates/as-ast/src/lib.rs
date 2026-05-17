//! Tree-sitter span extractor.
//!
//! Given a byte buffer and a byte offset (where a grep hit landed), return
//! the enclosing function / class / method as a `Span`. Falls back to the
//! original line span if the language is unsupported or the parser fails.
//!
//! Shipping languages (M2): Rust, Python, JavaScript, TypeScript, Go.

use as_core::Result;
use as_grep::{Span, SpanKind};
use tree_sitter::{Language, Node, Parser};

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
            name_kinds: &[
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
            name_kinds: &["function_definition", "class_definition"],
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
            name_kinds: &[
                "function_declaration",
                "method_definition",
                "class_declaration",
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
            name_kinds: &[
                "function_declaration",
                "method_definition",
                "class_declaration",
                "interface_declaration",
                "type_alias_declaration",
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
            name_kinds: &[
                "function_declaration",
                "method_definition",
                "class_declaration",
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
            name_kinds: &[
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
    pub name_kinds: &'static [&'static str],
}

/// Widen a (Line) span to its enclosing function/class/method using
/// tree-sitter. Returns the original span untouched if no AST mapping
/// applies.
pub fn widen_to_definition(bytes: &[u8], span: Span) -> Result<Span> {
    let lang = match language_for_uri(&span.uri) {
        Some(l) => l,
        None => return Ok(span),
    };
    let mut parser = Parser::new();
    if parser.set_language(&lang.language).is_err() {
        return Ok(span);
    }
    let tree = match parser.parse(bytes, None) {
        Some(t) => t,
        None => return Ok(span),
    };
    // The grep span starts at the beginning of a line (which is whitespace
    // for indented languages); tree-sitter does not give us a useful node
    // for whitespace. Skip past leading spaces / tabs to land on a real
    // token before walking up.
    let line_start = span.byte_range.start as usize;
    let line_end = (span.byte_range.end as usize).min(bytes.len());
    let probe = first_non_whitespace(bytes, line_start, line_end);
    let root = tree.root_node();
    let target = match root.descendant_for_byte_range(probe, probe.saturating_add(1)) {
        Some(n) => n,
        None => return Ok(span),
    };
    let definition = match enclosing_container(target, lang.container_kinds) {
        Some(n) => n,
        None => return Ok(span),
    };

    let start_byte = definition.start_byte() as u64;
    let end_byte = definition.end_byte() as u64;
    let line_start = (definition.start_position().row + 1) as u32;
    let line_end = (definition.end_position().row + 1) as u32;
    let symbol = symbol_name(definition, bytes);

    let kind = classify_kind(definition.kind());

    let snippet = bytes.get(start_byte as usize..end_byte as usize).map(|s| {
        String::from_utf8_lossy(s)
            .chars()
            .take(800)
            .collect::<String>()
    });

    Ok(Span {
        uri: span.uri,
        byte_range: start_byte..end_byte,
        line_range: [line_start, line_end],
        symbol,
        kind,
        snippet,
        score: span.score,
    })
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

fn enclosing_container<'a>(node: Node<'a>, kinds: &[&str]) -> Option<Node<'a>> {
    let mut cur = Some(node);
    while let Some(n) = cur {
        if kinds.iter().any(|k| *k == n.kind()) {
            return Some(n);
        }
        cur = n.parent();
    }
    None
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
    fn falls_through_for_unknown_extension() {
        let span = Span {
            uri: "file:///x.unknown".into(),
            byte_range: 0..1,
            line_range: [1, 1],
            symbol: None,
            kind: SpanKind::Line,
            snippet: None,
            score: 1.0,
        };
        let out = widen_to_definition(b"hello", span.clone()).unwrap();
        assert_eq!(out.kind, SpanKind::Line);
    }
}
