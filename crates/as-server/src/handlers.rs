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

pub async fn search(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<GrepResponse>, AppError> {
    let grep_req = GrepRequest {
        uri: req.uri,
        pattern: regex_escape(&req.query),
        case_insensitive: true,
        max_hits: req.k,
        concurrency: 32,
        ast: true,
    };
    grep(State(state), Json(grep_req)).await
}

async fn widen_spans(fs: &Arc<Fs>, spans: Vec<Span>) -> anyhow::Result<Vec<Span>> {
    use std::collections::{HashMap, HashSet};
    let mut by_uri: HashMap<String, Vec<Span>> = HashMap::new();
    for s in spans {
        by_uri.entry(s.uri.clone()).or_default().push(s);
    }
    let mut out: Vec<Span> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (uri, mut group) in by_uri {
        let bytes = match fs.read(&uri).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        widen_many(&bytes, &mut group)?;
        for s in group {
            if seen.insert(s.dedup_key()) {
                out.push(s);
            }
        }
    }
    Ok(out)
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
