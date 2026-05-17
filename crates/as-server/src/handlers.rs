//! REST handlers. Each handler maps one tool surface call to the planner
//! / FS layer. Inputs and outputs are intentionally minimal; the MCP
//! bridge in `mcp_stdio` adapts these same JSON shapes for stdio clients.

use crate::AppState;
use as_ast::widen_many;
use as_fs::Fs;
use as_grep::{GrepOpts, ParallelGrep, ParallelOpts, Span};
use axum::{extract::State, http::StatusCode, Json};
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub async fn health() -> &'static str {
    "ok"
}

#[derive(Debug, Deserialize)]
pub struct LsRequest {
    pub uri: String,
    #[serde(default)]
    pub glob: Option<String>,
    #[serde(default = "default_ls_limit")]
    pub limit: usize,
}
fn default_ls_limit() -> usize {
    1000
}

#[derive(Debug, Serialize)]
pub struct LsEntry {
    pub key: String,
    pub size: u64,
}

#[derive(Debug, Serialize)]
pub struct LsResponse {
    pub entries: Vec<LsEntry>,
}

pub async fn ls(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LsRequest>,
) -> Result<Json<LsResponse>, AppError> {
    let (fs, prefix) = state.open_fs(&req.uri)?;
    let mut stream = match req.glob.as_deref() {
        Some(pat) => fs.glob(&prefix, pat).map_err(AppError::from)?,
        None => fs.list(&prefix),
    };
    let mut entries = Vec::new();
    while let Some(item) = stream.next().await {
        let m = item.map_err(AppError::from)?;
        entries.push(LsEntry {
            key: m.key,
            size: m.size,
        });
        if entries.len() >= req.limit {
            break;
        }
    }
    Ok(Json(LsResponse { entries }))
}

#[derive(Debug, Deserialize)]
pub struct ReadRequest {
    pub uri: String,
    #[serde(default)]
    pub offset: Option<u64>,
    #[serde(default)]
    pub length: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct ReadResponse {
    pub uri: String,
    pub bytes: usize,
    pub text: Option<String>,
}

pub async fn read(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ReadRequest>,
) -> Result<Json<ReadResponse>, AppError> {
    let (fs, key) = state.open_fs(&req.uri)?;
    let data = match (req.offset, req.length) {
        (Some(o), Some(l)) => fs
            .read_at(&key, o..o.saturating_add(l))
            .await
            .map_err(AppError::from)?,
        (None, None) => fs.read(&key).await.map_err(AppError::from)?,
        _ => {
            return Err(AppError::bad_request(
                "offset and length must be provided together",
            ))
        }
    };
    let text = std::str::from_utf8(&data).ok().map(|s| s.to_string());
    Ok(Json(ReadResponse {
        uri: req.uri,
        bytes: data.len(),
        text,
    }))
}

#[derive(Debug, Deserialize)]
pub struct GrepRequest {
    pub uri: String,
    pub pattern: String,
    #[serde(default)]
    pub case_insensitive: bool,
    #[serde(default = "default_max_hits")]
    pub max_hits: usize,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    /// Widen each hit to its enclosing function/class/method.
    #[serde(default)]
    pub ast: bool,
}
fn default_max_hits() -> usize {
    1000
}
fn default_concurrency() -> usize {
    32
}

#[derive(Debug, Serialize)]
pub struct GrepResponse {
    pub spans: Vec<Span>,
}

pub async fn grep(
    State(state): State<Arc<AppState>>,
    Json(req): Json<GrepRequest>,
) -> Result<Json<GrepResponse>, AppError> {
    let (fs, prefix) = state.open_fs(&req.uri)?;
    let opts = ParallelOpts {
        grep: GrepOpts {
            case_insensitive: req.case_insensitive,
            multi_line: false,
            max_hits_per_file: None,
        },
        concurrency: req.concurrency,
        max_object_bytes: 64 * 1024 * 1024,
        max_total_spans: Some(req.max_hits),
    };
    let pg = ParallelGrep::new(fs.clone());
    let spans = pg.scan_prefix(&prefix, &req.pattern, &opts).await?;
    let spans = if req.ast {
        widen_spans(&fs, spans).await?
    } else {
        spans
    };
    Ok(Json(GrepResponse { spans }))
}

#[derive(Debug, Deserialize)]
pub struct FindRequest {
    pub uri: String,
    pub symbol: String,
    #[serde(default = "default_max_hits")]
    pub max_hits: usize,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
}

pub async fn find(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FindRequest>,
) -> Result<Json<GrepResponse>, AppError> {
    let pattern = format!(r"\b{}\b", regex_escape(&req.symbol));
    let symbol = req.symbol.clone();
    let grep_req = GrepRequest {
        uri: req.uri,
        pattern,
        case_insensitive: false,
        // Overshoot the max so we have slack after AST verification drops
        // call sites / comment hits / partial matches.
        max_hits: req.max_hits.saturating_mul(4),
        concurrency: req.concurrency,
        ast: true,
    };
    let raw = grep(State(state), Json(grep_req)).await?;
    // Only keep AST-widened spans whose tree-sitter name field matches the
    // exact requested symbol. This drops comment / string / call-site hits
    // that grep alone surfaces.
    let mut spans = raw.0.spans;
    spans.retain(|s| s.symbol.as_deref() == Some(symbol.as_str()));
    spans.truncate(req.max_hits);
    Ok(Json(GrepResponse { spans }))
}

/// `/search` is the planner-fronted endpoint. v1 implements it as grep+AST
/// (the only on-default stages); future versions can mix in optional
/// vector / index / web stages.
#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub uri: String,
    pub query: String,
    #[serde(default = "default_k")]
    pub k: usize,
}
fn default_k() -> usize {
    20
}

#[derive(Debug, Deserialize)]
pub struct DelegateRequest {
    pub uri: String,
    pub query: String,
    #[serde(default = "default_k")]
    pub k: usize,
    /// Max wall-time budget the subagent loop may spend. Default 5s.
    #[serde(default = "default_delegate_budget_ms")]
    pub budget_ms: u64,
}
fn default_delegate_budget_ms() -> u64 {
    5000
}

#[derive(Debug, Serialize)]
pub struct DelegateFinding {
    pub uri: String,
    pub line_range: [u32; 2],
    pub byte_range: [u64; 2],
    pub symbol: Option<String>,
    pub kind: as_grep::SpanKind,
    pub snippet: String,
}

#[derive(Debug, Serialize)]
pub struct DelegateResponse {
    pub summary: String,
    pub findings: Vec<DelegateFinding>,
    pub stats: serde_json::Value,
}

/// /delegate — search-only subagent loop. Runs the same planner as
/// /search but with a larger candidate budget, then compresses results
/// into a token-frugal answer with citations. Designed to be called by
/// a lead agent that wants a single tool that answers "find X" cheaply.
pub async fn delegate(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DelegateRequest>,
) -> Result<Json<DelegateResponse>, AppError> {
    let started = std::time::Instant::now();
    let k = req.k.clamp(1, 50);
    let candidate_limit = k.saturating_mul(20).clamp(64, 2000);
    let terms = query_terms(&req.query);
    let stats_terms: Vec<String> = terms.to_vec();
    if terms.is_empty() {
        return Ok(Json(DelegateResponse {
            summary: "query has no useful terms; nothing to delegate".into(),
            findings: vec![],
            stats: serde_json::json!({"terms": stats_terms, "elapsed_ms": started.elapsed().as_millis()}),
        }));
    }
    let pattern = terms
        .iter()
        .map(|t| regex_escape(t))
        .collect::<Vec<_>>()
        .join("|");
    let grep_req = GrepRequest {
        uri: req.uri,
        pattern,
        case_insensitive: true,
        max_hits: candidate_limit,
        concurrency: 32,
        ast: true,
    };
    let budget = std::time::Duration::from_millis(req.budget_ms);
    let raw = match tokio::time::timeout(budget, grep(State(state), Json(grep_req))).await {
        Ok(r) => r?,
        Err(_) => {
            return Ok(Json(DelegateResponse {
                summary: format!("budget {}ms exceeded before any results", req.budget_ms),
                findings: vec![],
                stats: serde_json::json!({"terms": stats_terms, "timeout": true}),
            }));
        }
    };
    let ranked = rank_search_spans(raw.0.spans, &req.query, &terms, k);
    let total = ranked.len();
    let elapsed = started.elapsed().as_millis();
    let findings: Vec<DelegateFinding> = ranked
        .into_iter()
        .map(|s| DelegateFinding {
            line_range: s.line_range,
            byte_range: [s.byte_range.start, s.byte_range.end],
            symbol: s.symbol,
            kind: s.kind,
            snippet: s
                .snippet
                .as_deref()
                .map(|t| {
                    // Compress: first non-empty line, capped at 240 chars.
                    let line = t.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                    line.chars().take(240).collect::<String>()
                })
                .unwrap_or_default(),
            uri: s.uri,
        })
        .collect();
    let summary = if findings.is_empty() {
        format!("no matches for {:?}", req.query)
    } else {
        let by_uri: std::collections::BTreeMap<&str, usize> =
            findings
                .iter()
                .fold(std::collections::BTreeMap::new(), |mut acc, f| {
                    *acc.entry(f.uri.as_str()).or_insert(0) += 1;
                    acc
                });
        let breakdown: Vec<String> = by_uri
            .iter()
            .map(|(uri, n)| format!("{n} in {uri}"))
            .collect();
        format!(
            "{total} match(es) for {:?}: {}",
            req.query,
            breakdown.join(", ")
        )
    };
    Ok(Json(DelegateResponse {
        summary,
        findings,
        stats: serde_json::json!({
            "terms": stats_terms,
            "elapsed_ms": elapsed,
            "budget_ms": req.budget_ms,
        }),
    }))
}

pub async fn search(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<GrepResponse>, AppError> {
    if req.k == 0 {
        return Ok(Json(GrepResponse { spans: Vec::new() }));
    }
    let terms = query_terms(&req.query);
    if terms.is_empty() {
        return Ok(Json(GrepResponse { spans: Vec::new() }));
    }
    let pattern = terms
        .iter()
        .map(|t| regex_escape(t))
        .collect::<Vec<_>>()
        .join("|");
    let candidate_limit = req.k.saturating_mul(8).clamp(64, 2000);
    let grep_req = GrepRequest {
        uri: req.uri,
        pattern,
        case_insensitive: true,
        max_hits: candidate_limit,
        concurrency: 32,
        ast: true,
    };
    let raw = grep(State(state), Json(grep_req)).await?;
    Ok(Json(GrepResponse {
        spans: rank_search_spans(raw.0.spans, &req.query, &terms, req.k),
    }))
}

async fn widen_spans(fs: &Arc<Fs>, spans: Vec<Span>) -> anyhow::Result<Vec<Span>> {
    use std::collections::{BTreeMap, HashSet};
    let mut by_uri: BTreeMap<String, Vec<Span>> = BTreeMap::new();
    for s in spans {
        by_uri.entry(s.uri.clone()).or_default().push(s);
    }
    let mut pending = futures::stream::FuturesUnordered::new();
    let mut out: Vec<Span> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (uri, group) in by_uri {
        let fs = fs.clone();
        pending.push(tokio::spawn(async move {
            let bytes = fs.read(&uri).await?;
            tokio::task::spawn_blocking(move || {
                let mut group = group;
                widen_many(&bytes, &mut group)?;
                Ok::<_, anyhow::Error>(group)
            })
            .await
            .map_err(|e| anyhow::anyhow!("ast join: {e}"))?
        }));
        if pending.len() >= 16 {
            drain_widened(&mut pending, &mut out, &mut seen).await?;
        }
    }
    while !pending.is_empty() {
        drain_widened(&mut pending, &mut out, &mut seen).await?;
    }
    out.sort_by(|a, b| {
        a.uri
            .cmp(&b.uri)
            .then(a.line_range[0].cmp(&b.line_range[0]))
            .then(a.byte_range.start.cmp(&b.byte_range.start))
    });
    Ok(out)
}

async fn drain_widened(
    pending: &mut futures::stream::FuturesUnordered<
        tokio::task::JoinHandle<anyhow::Result<Vec<Span>>>,
    >,
    out: &mut Vec<Span>,
    seen: &mut std::collections::HashSet<String>,
) -> anyhow::Result<()> {
    if let Some(joined) = pending.next().await {
        let group = joined.map_err(|e| anyhow::anyhow!("widen task join: {e}"))??;
        for s in group {
            if seen.insert(s.dedup_key()) {
                out.push(s);
            }
        }
    }
    Ok(())
}

fn query_terms(query: &str) -> Vec<String> {
    use std::collections::BTreeSet;
    let raw: Vec<String> = query
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .filter_map(|t| {
            let t = t.trim().to_ascii_lowercase();
            (t.len() >= 2).then_some(t)
        })
        .collect();
    let mut filtered = BTreeSet::new();
    for t in &raw {
        if !is_stopword(t) {
            filtered.insert(t.clone());
        }
    }
    if filtered.is_empty() {
        raw.into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    } else {
        filtered.into_iter().take(16).collect()
    }
}

fn is_stopword(s: &str) -> bool {
    matches!(
        s,
        "find"
            | "search"
            | "show"
            | "inside"
            | "within"
            | "function"
            | "method"
            | "class"
            | "symbol"
            | "file"
            | "files"
            | "the"
            | "and"
            | "or"
            | "with"
            | "from"
            | "for"
            | "into"
            | "where"
            | "that"
            | "this"
            | "use"
            | "uses"
            | "using"
    )
}

fn rank_search_spans(mut spans: Vec<Span>, query: &str, terms: &[String], k: usize) -> Vec<Span> {
    let phrase = query.to_ascii_lowercase();
    for span in &mut spans {
        let haystack = format!(
            "{}\n{}\n{}",
            span.uri,
            span.symbol.as_deref().unwrap_or(""),
            span.snippet.as_deref().unwrap_or("")
        )
        .to_ascii_lowercase();
        let term_hits = terms
            .iter()
            .filter(|t| haystack.contains(t.as_str()))
            .count();
        let symbol_hits = span
            .symbol
            .as_deref()
            .map(|sym| {
                let sym = sym.to_ascii_lowercase();
                terms.iter().filter(|t| sym == **t).count()
            })
            .unwrap_or(0);
        let phrase_boost = (!phrase.is_empty() && haystack.contains(&phrase)) as u8 as f32;
        span.score = term_hits as f32 + (symbol_hits as f32 * 0.75) + phrase_boost;
    }
    spans.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.uri.cmp(&b.uri))
            .then(a.line_range[0].cmp(&b.line_range[0]))
            .then(a.byte_range.start.cmp(&b.byte_range.start))
    });
    spans.truncate(k);
    spans
}

fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '.' | '+'
                | '*'
                | '?'
                | '('
                | ')'
                | '|'
                | '['
                | ']'
                | '{'
                | '}'
                | '^'
                | '$'
                | '\\'
                | '/'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[derive(Debug)]
pub struct AppError(pub StatusCode, pub String);

impl AppError {
    pub fn bad_request(s: impl Into<String>) -> Self {
        Self(StatusCode::BAD_REQUEST, s.into())
    }
}

impl<E: std::fmt::Display> From<E> for AppError {
    fn from(e: E) -> Self {
        Self(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
    }
}

impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let body = serde_json::json!({ "error": self.1 });
        (self.0, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, sync::Arc};
    use tempfile::tempdir;

    #[tokio::test]
    async fn search_tokenizes_and_ranks_natural_language_query() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("a.py"),
            "def alpha(x):\n    return x + 1\n\n\
             def beta(x):\n    # TODO: optimize beta\n    return x * 2\n",
        )
        .unwrap();
        let uri = format!("file://{}", dir.path().display());
        let state = Arc::new(AppState::default());

        let resp = search(
            State(state),
            Json(SearchRequest {
                uri,
                query: "find TODO inside beta function".to_string(),
                k: 5,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.0.spans[0].symbol.as_deref(), Some("beta"));
        assert!(resp.0.spans[0].snippet.as_deref().unwrap().contains("TODO"));
    }

    #[tokio::test]
    async fn find_keeps_only_exact_ast_symbol_matches() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("a.py"),
            "def beta(x):\n    return x * 2\n\n\
             def caller():\n    beta(1)\n    # beta mention\n",
        )
        .unwrap();
        let uri = format!("file://{}", dir.path().display());
        let state = Arc::new(AppState::default());

        let resp = find(
            State(state),
            Json(FindRequest {
                uri,
                symbol: "beta".to_string(),
                max_hits: 10,
                concurrency: 4,
            }),
        )
        .await
        .unwrap();

        assert_eq!(resp.0.spans.len(), 1);
        assert_eq!(resp.0.spans[0].symbol.as_deref(), Some("beta"));
        assert_eq!(resp.0.spans[0].line_range, [1, 2]);
    }
}
