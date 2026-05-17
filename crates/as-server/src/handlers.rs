//! REST handlers. Each handler maps one tool surface call to the planner
//! / FS layer. Inputs and outputs are intentionally minimal; the MCP
//! bridge in `mcp_stdio` adapts these same JSON shapes for stdio clients.

use crate::AppState;
use as_ast::{widen_with_cache_cancellable, SpanCache};
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

/// Output verbosity for span-returning endpoints. The wire shape is
/// always JSON; `response_format` chooses *what* lives inside.
///
/// - `concise`  → minimal token surface: uri, line_range, kind,
///   symbol (if any), one-line snippet capped at 160 chars. Designed
///   for the lead-agent loop where the model only needs a citation.
/// - `detailed` → full `Span` struct including rank_signals,
///   source_stage, content_hash, etc. The default.
/// - `jsonl`    → newline-delimited JSON objects packed into a single
///   string so the agent can stream-parse them and chunk for tool
///   budgets without re-running search.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFormat {
    Concise,
    #[default]
    Detailed,
    Jsonl,
}

#[derive(Debug, Serialize)]
pub struct ConciseSpan {
    pub uri: String,
    pub line_range: [u32; 2],
    pub kind: as_grep::SpanKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    pub snippet: String,
}

impl ConciseSpan {
    fn from(s: &Span) -> Self {
        let snippet = s
            .snippet
            .as_deref()
            .map(|raw| {
                raw.lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .chars()
                    .take(160)
                    .collect::<String>()
            })
            .unwrap_or_default();
        Self {
            uri: s.uri.clone(),
            line_range: s.line_range,
            kind: s.kind,
            symbol: s.symbol.clone(),
            snippet,
        }
    }
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
    #[serde(default)]
    pub response_format: ResponseFormat,
}
fn default_max_hits() -> usize {
    1000
}
fn default_concurrency() -> usize {
    32
}

/// Envelope for span-returning endpoints. Exactly one of `spans`,
/// `concise`, or `jsonl` is populated based on the request's
/// `response_format`.
#[derive(Debug, Serialize)]
pub struct GrepResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spans: Option<Vec<Span>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concise: Option<Vec<ConciseSpan>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jsonl: Option<String>,
    pub format: &'static str,
}

impl GrepResponse {
    pub fn from_spans(spans: Vec<Span>, fmt: ResponseFormat) -> Self {
        match fmt {
            ResponseFormat::Detailed => Self {
                spans: Some(spans),
                concise: None,
                jsonl: None,
                format: "detailed",
            },
            ResponseFormat::Concise => Self {
                concise: Some(spans.iter().map(ConciseSpan::from).collect()),
                spans: None,
                jsonl: None,
                format: "concise",
            },
            ResponseFormat::Jsonl => {
                let mut buf = String::new();
                for s in &spans {
                    if let Ok(line) = serde_json::to_string(s) {
                        buf.push_str(&line);
                        buf.push('\n');
                    }
                }
                Self {
                    jsonl: Some(buf),
                    spans: None,
                    concise: None,
                    format: "jsonl",
                }
            }
        }
    }
}

pub async fn grep(
    State(state): State<Arc<AppState>>,
    Json(req): Json<GrepRequest>,
) -> Result<Json<GrepResponse>, AppError> {
    let fmt = req.response_format;
    let spans = run_grep(state, &req).await?;
    Ok(Json(GrepResponse::from_spans(spans, fmt)))
}

/// Caps for request-body numbers so a single client can't trigger
/// runaway concurrency or memory just by sending big integers. The
/// cap is generous (covers every realistic agent workload) but
/// finite — these are the public limits we will document.
const MAX_CONCURRENCY: usize = 256;
const MAX_HITS_CAP: usize = 5_000;

/// Internal helper so other handlers can reuse the grep + AST stage
/// without re-encoding into the public envelope.
async fn run_grep(state: Arc<AppState>, req: &GrepRequest) -> Result<Vec<Span>, AppError> {
    let (fs, prefix) = state.open_fs(&req.uri)?;
    let opts = ParallelOpts {
        grep: GrepOpts {
            case_insensitive: req.case_insensitive,
            multi_line: false,
            max_hits_per_file: None,
        },
        concurrency: req.concurrency.clamp(1, MAX_CONCURRENCY),
        max_object_bytes: 64 * 1024 * 1024,
        max_total_spans: Some(req.max_hits.clamp(1, MAX_HITS_CAP)),
    };
    let pg = ParallelGrep::new(fs.clone());
    let spans = pg.scan_prefix(&prefix, &req.pattern, &opts).await?;
    if req.ast {
        Ok(widen_spans(&fs, &state.ast, spans).await?)
    } else {
        Ok(spans)
    }
}

#[derive(Debug, Deserialize)]
pub struct FindRequest {
    pub uri: String,
    pub symbol: String,
    #[serde(default = "default_max_hits")]
    pub max_hits: usize,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    #[serde(default)]
    pub response_format: ResponseFormat,
}

pub async fn find(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FindRequest>,
) -> Result<Json<GrepResponse>, AppError> {
    // Expand the requested symbol across the common naming conventions
    // (camelCase, snake_case, kebab-case, SCREAMING_SNAKE_CASE, PascalCase)
    // so `/find foo_bar` also picks up `fooBar` and `FOO_BAR`. The
    // grep stage runs on the alternation; the AST post-filter keeps
    // only spans whose tree-sitter name field is one of the variants.
    let variants = symbol_case_variants(&req.symbol);
    let pattern = variants
        .iter()
        .map(|v| format!(r"\b{}\b", regex_escape(v)))
        .collect::<Vec<_>>()
        .join("|");
    let fmt = req.response_format;
    let grep_req = GrepRequest {
        uri: req.uri,
        pattern,
        case_insensitive: false,
        // Overshoot the max so we have slack after AST verification drops
        // call sites / comment hits / partial matches.
        max_hits: req.max_hits.saturating_mul(4),
        concurrency: req.concurrency,
        ast: true,
        response_format: ResponseFormat::Detailed,
    };
    let mut spans = run_grep(state, &grep_req).await?;
    let variant_set: std::collections::HashSet<&str> =
        variants.iter().map(|s| s.as_str()).collect();
    // Only keep AST-widened spans whose tree-sitter name field matches
    // any variant of the requested symbol. Drops comment / string /
    // call-site hits that grep alone surfaces.
    spans.retain(|s| {
        s.symbol
            .as_deref()
            .map(|sym| variant_set.contains(sym))
            .unwrap_or(false)
    });
    spans.truncate(req.max_hits);
    Ok(Json(GrepResponse::from_spans(spans, fmt)))
}

/// Expand a symbol name into every common code-style variant so the
/// grep stage finds matches regardless of how the codebase spells it.
/// Returns the original *plus* the variants, deduplicated, in stable
/// order. Trivial / one-char names short-circuit to just themselves.
pub(crate) fn symbol_case_variants(symbol: &str) -> Vec<String> {
    if symbol.chars().count() < 2 {
        return vec![symbol.to_string()];
    }
    // 1. Split into lowercase word tokens regardless of input casing.
    let mut words: Vec<String> = Vec::new();
    let mut cur = String::new();
    let push_cur = |cur: &mut String, words: &mut Vec<String>| {
        if !cur.is_empty() {
            words.push(std::mem::take(cur).to_ascii_lowercase());
        }
    };
    let chars: Vec<char> = symbol.chars().collect();
    let mut prev: Option<char> = None;
    for (i, &c) in chars.iter().enumerate() {
        if c == '_' || c == '-' || c == '.' || c == ':' {
            push_cur(&mut cur, &mut words);
            prev = None;
            continue;
        }
        if let Some(p) = prev {
            // Boundary A: lower/digit → upper. Splits "fooBar" / "v2Api".
            let camel_boundary =
                (p.is_ascii_lowercase() || p.is_ascii_digit()) && c.is_ascii_uppercase();
            // Boundary B: end-of-acronym. Two adjacent uppercase letters
            // where the *next* char is lowercase mean we just left an
            // acronym and entered a new TitleCase word, e.g.
            // "HTTPServer" → ["HTTP", "Server"], "XMLHttpReq" →
            // ["XML", "Http", "Req"]. Without this we'd see one giant
            // word "httpserver" and miss case variants.
            let acronym_boundary = p.is_ascii_uppercase()
                && c.is_ascii_uppercase()
                && chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
            if camel_boundary || acronym_boundary {
                push_cur(&mut cur, &mut words);
            }
        }
        cur.push(c);
        prev = Some(c);
    }
    push_cur(&mut cur, &mut words);
    if words.is_empty() {
        return vec![symbol.to_string()];
    }
    // Helper to title-case a single ASCII word.
    let title = |w: &str| -> String {
        let mut chars = w.chars();
        match chars.next() {
            None => String::new(),
            Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
        }
    };
    let snake = words.join("_");
    let kebab = words.join("-");
    let scream = snake.to_ascii_uppercase();
    let pascal: String = words.iter().map(|w| title(w)).collect();
    let camel = {
        let mut it = words.iter();
        let first = it.next().cloned().unwrap_or_default();
        let rest: String = it.map(|w| title(w)).collect();
        first + &rest
    };
    let mut out: Vec<String> = vec![symbol.to_string(), snake, kebab, scream, camel, pascal];
    // De-dup preserving order.
    let mut seen = std::collections::HashSet::new();
    out.retain(|v| !v.is_empty() && seen.insert(v.clone()));
    out
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
    #[serde(default)]
    pub response_format: ResponseFormat,
    /// Endpoint-wide wall-time ceiling. Defaults to 15s; clamped to
    /// `MAX_SEARCH_BUDGET_MS`. Lets agents bound a hopeless query
    /// without the server triple-scanning the corpus.
    #[serde(default = "default_search_budget_ms")]
    pub budget_ms: u64,
}
fn default_search_budget_ms() -> u64 {
    15_000
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

/// Hard ceiling on `/delegate` wall-time. A misbehaving (or
/// adversarial) caller cannot disable the guard with an arbitrarily
/// large `budget_ms`; we silently clamp.
const MAX_DELEGATE_BUDGET_MS: u64 = 30_000;

/// Hard ceiling on `/search` wall-time. `/search` already runs its
/// stages with per-stage budgets, but those budgets stack across the
/// three parallel probes plus AST widening. This is the endpoint-wide
/// cap so a no-hit pathological query cannot triple corpus IO
/// indefinitely.
const MAX_SEARCH_BUDGET_MS: u64 = 30_000;

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
        response_format: ResponseFormat::Detailed,
    };
    let budget_ms = req.budget_ms.min(MAX_DELEGATE_BUDGET_MS);
    let budget = std::time::Duration::from_millis(budget_ms);
    let raw_spans = match tokio::time::timeout(budget, run_grep(state, &grep_req)).await {
        Ok(r) => r?,
        Err(_) => {
            return Ok(Json(DelegateResponse {
                summary: format!("budget {}ms exceeded before any results", budget_ms),
                findings: vec![],
                stats: serde_json::json!({"terms": stats_terms, "timeout": true}),
            }));
        }
    };
    let ranked = rank_search_spans(raw_spans, &req.query, &terms, k);
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
            "budget_ms": budget_ms,
            "requested_budget_ms": req.budget_ms,
        }),
    }))
}

pub async fn search(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<GrepResponse>, AppError> {
    let fmt = req.response_format;
    if req.k == 0 {
        return Ok(Json(GrepResponse::from_spans(Vec::new(), fmt)));
    }
    let terms = query_terms(&req.query);
    if terms.is_empty() {
        return Ok(Json(GrepResponse::from_spans(Vec::new(), fmt)));
    }

    let budget_ms = req.budget_ms.min(MAX_SEARCH_BUDGET_MS);
    let budget = std::time::Duration::from_millis(budget_ms);
    match tokio::time::timeout(budget, search_inner(state, req, terms, fmt)).await {
        Ok(r) => r,
        Err(_) => Ok(Json(GrepResponse::from_spans(Vec::new(), fmt))),
    }
}

async fn search_inner(
    state: Arc<AppState>,
    req: SearchRequest,
    terms: Vec<String>,
    fmt: ResponseFormat,
) -> Result<Json<GrepResponse>, AppError> {

    // Fan out *multiple* grep probes in parallel and fuse spans rather
    // than running a single OR-of-terms regex. The model picks one
    // shape from the query (one "find" intent) but a single English
    // phrase typically maps to several useful patterns:
    //
    //   - OR-of-tokens  (broad recall; what we had before)
    //   - phrase        (literal whole-phrase match for exact quoting)
    //   - first-term    (focused on the most distinctive token)
    //
    // Spans from the three probes are merged and re-ranked, capped at
    // `k`. Probes that error out are dropped silently — the planner
    // should never fail the whole call because one regex was odd.
    let candidate_limit = req.k.saturating_mul(8).clamp(64, 2000);
    let mut probes: Vec<String> = Vec::with_capacity(3);
    let union = terms
        .iter()
        .map(|t| regex_escape(t))
        .collect::<Vec<_>>()
        .join("|");
    probes.push(union);
    let phrase = req.query.trim();
    if phrase.split_whitespace().count() > 1 {
        probes.push(regex_escape(phrase));
    }
    if let Some(first) = terms.first() {
        let term0 = regex_escape(first);
        if !probes.iter().any(|p| p == &term0) {
            probes.push(term0);
        }
    }

    // Fan out the probes in parallel — the previous loop awaited each
    // probe before starting the next, so the "multi-probe" planner was
    // effectively serial. JoinSet drops cancel everything on early
    // return (e.g. a /delegate timeout dropping this future).
    let mut probe_set: tokio::task::JoinSet<Result<Vec<Span>, AppError>> =
        tokio::task::JoinSet::new();
    for pattern in &probes {
        let grep_req = GrepRequest {
            uri: req.uri.clone(),
            pattern: pattern.clone(),
            case_insensitive: true,
            max_hits: candidate_limit,
            concurrency: 32,
            ast: true,
            response_format: ResponseFormat::Detailed,
        };
        let state = state.clone();
        probe_set.spawn(async move { run_grep(state, &grep_req).await });
    }
    let mut joined: Vec<Vec<Span>> = Vec::with_capacity(probes.len());
    while let Some(joined_res) = probe_set.join_next().await {
        match joined_res {
            Ok(Ok(spans)) => joined.push(spans),
            // Per-probe failure (e.g. odd regex) is silently dropped so
            // one bad probe never poisons the whole call.
            Ok(Err(_)) | Err(_) => continue,
        }
    }

    // Dedup by span key, accumulate scores via RRF, then re-rank by
    // per-term overlap so phrase matches win over noisy single-token
    // hits in ties.
    let fused = as_plan::rrf(&joined, 60, candidate_limit);
    let ranked = rank_search_spans(fused, &req.query, &terms, req.k);
    Ok(Json(GrepResponse::from_spans(ranked, fmt)))
}

async fn widen_spans(
    fs: &Arc<Fs>,
    ast_cache: &Arc<SpanCache>,
    spans: Vec<Span>,
) -> anyhow::Result<Vec<Span>> {
    use std::collections::{BTreeMap, HashSet};
    use std::sync::atomic::{AtomicBool, Ordering};
    let mut by_uri: BTreeMap<String, Vec<Span>> = BTreeMap::new();
    for s in spans {
        by_uri.entry(s.uri.clone()).or_default().push(s);
    }
    // `JoinSet` drops abort spawned *async* tasks, but a `spawn_blocking`
    // job already running on the blocking pool keeps executing after
    // its async wrapper is dropped. The shared `cancelled` flag is
    // raised by `CancelGuard::drop` when this future is cancelled
    // (timeout, client disconnect, …); each `widen_with_cache_cancellable`
    // checks it between span iterations and exits early.
    struct CancelGuard(Arc<AtomicBool>);
    impl Drop for CancelGuard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let _guard = CancelGuard(cancelled.clone());

    let mut pending: tokio::task::JoinSet<anyhow::Result<Vec<Span>>> = tokio::task::JoinSet::new();
    let mut out: Vec<Span> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (uri, group) in by_uri {
        let fs = fs.clone();
        let cache = ast_cache.clone();
        let cancelled = cancelled.clone();
        pending.spawn(async move {
            // Cheap fast-path: don't even read the file if we were
            // already cancelled before this future got scheduled.
            if cancelled.load(Ordering::Acquire) {
                return Ok(Vec::new());
            }
            let bytes = fs.read(&uri).await?;
            tokio::task::spawn_blocking(move || {
                let mut group = group;
                widen_with_cache_cancellable(&cache, &bytes, &mut group, &|| {
                    cancelled.load(Ordering::Acquire)
                })?;
                Ok::<_, anyhow::Error>(group)
            })
            .await
            .map_err(|e| anyhow::anyhow!("ast join: {e}"))?
        });
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
    pending: &mut tokio::task::JoinSet<anyhow::Result<Vec<Span>>>,
    out: &mut Vec<Span>,
    seen: &mut std::collections::HashSet<String>,
) -> anyhow::Result<()> {
    if let Some(joined) = pending.join_next().await {
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
    use std::collections::HashSet;
    let raw: Vec<String> = query
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .filter_map(|t| {
            let t = t.trim().to_ascii_lowercase();
            (t.len() >= 2).then_some(t)
        })
        .collect();
    // Preserve query order: the first non-stopword in the query is the
    // model's intent anchor (used by the "first-term" probe in
    // `/search`); alphabetically sorting via BTreeSet threw that away.
    let mut seen: HashSet<String> = HashSet::new();
    let mut filtered: Vec<String> = Vec::new();
    for t in &raw {
        if is_stopword(t) {
            continue;
        }
        if seen.insert(t.clone()) {
            filtered.push(t.clone());
        }
    }
    if filtered.is_empty() {
        // Last resort: surface query order without stopword filtering.
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for t in raw {
            if seen.insert(t.clone()) {
                out.push(t);
            }
        }
        out.truncate(16);
        out
    } else {
        filtered.truncate(16);
        filtered
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
    // We keep the RRF score (set by `as_plan::rrf`) as the primary
    // ranking signal — that's how multi-probe agreement is preserved.
    // Term-overlap is recorded into `rank_signals.term_overlap` and
    // used as a soft tiebreaker only.
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
        let lexical_boost = term_hits as f32 + (symbol_hits as f32 * 0.75) + phrase_boost;
        // RRF scores live in the ~[0.01, 0.05] range (1/(60+rank)),
        // so a +0.05 boost per term-hit overpowers the fused signal
        // and turns this into a lexical-only ranker. Use 0.001 — small
        // enough that ten lexical hits still trail one extra
        // multi-probe agreement, but non-zero so it breaks ties.
        const LEXICAL_BOOST_WEIGHT: f32 = 0.001;
        let signals = span.rank_signals.get_or_insert_with(Default::default);
        signals.literal_match = Some(phrase_boost);
        signals.term_overlap = Some(term_hits as u16);
        span.score += lexical_boost * LEXICAL_BOOST_WEIGHT;
    }
    spans.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| {
                let a_overlap = a
                    .rank_signals
                    .as_ref()
                    .and_then(|s| s.term_overlap)
                    .unwrap_or(0);
                let b_overlap = b
                    .rank_signals
                    .as_ref()
                    .and_then(|s| s.term_overlap)
                    .unwrap_or(0);
                b_overlap.cmp(&a_overlap)
            })
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

    #[test]
    fn symbol_case_variants_cover_common_styles() {
        let v = symbol_case_variants("fooBar");
        for want in ["fooBar", "FooBar", "foo_bar", "FOO_BAR", "foo-bar"] {
            assert!(v.contains(&want.to_string()), "missing {want}: {v:?}");
        }
    }

    #[test]
    fn symbol_case_variants_dedup_when_input_is_lowercase() {
        let v = symbol_case_variants("get");
        // single-word input collapses all styles to "get" / "Get" / "GET".
        assert!(v.contains(&"get".to_string()));
        assert!(v.contains(&"Get".to_string()));
        assert!(v.contains(&"GET".to_string()));
        // no duplicates
        let mut sorted = v.clone();
        sorted.sort();
        let mut deduped = sorted.clone();
        deduped.dedup();
        assert_eq!(sorted, deduped);
    }

    #[test]
    fn symbol_case_variants_handles_short_identifier() {
        let v = symbol_case_variants("x");
        assert_eq!(v, vec!["x".to_string()]);
    }

    #[test]
    fn symbol_case_variants_splits_at_acronym_boundary() {
        // HTTPServer must split between "HTTP" and "Server" so the
        // generated snake_case is "http_server", not "httpserver".
        let v = symbol_case_variants("HTTPServer");
        for want in [
            "HTTPServer",
            "http_server",
            "http-server",
            "HTTP_SERVER",
            "HttpServer",
            "httpServer",
        ] {
            assert!(v.contains(&want.to_string()), "missing {want}: {v:?}");
        }
    }

    #[test]
    fn symbol_case_variants_handles_pure_acronym() {
        // "HTTPS" has no following lowercase so should remain one word.
        let v = symbol_case_variants("HTTPS");
        assert!(v.contains(&"HTTPS".to_string()));
        assert!(v.contains(&"https".to_string()));
        assert!(!v.iter().any(|s| s.contains('_')));
    }

    #[test]
    fn symbol_case_variants_chained_acronym_then_word() {
        // XMLHttpReq → XML, Http, Req
        let v = symbol_case_variants("XMLHttpReq");
        assert!(v.contains(&"xml_http_req".to_string()), "{v:?}");
    }

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
                response_format: ResponseFormat::Detailed,
                budget_ms: 5_000,
            }),
        )
        .await
        .unwrap();

        let spans = resp.0.spans.expect("detailed format returns spans");
        assert_eq!(spans[0].symbol.as_deref(), Some("beta"));
        assert!(spans[0].snippet.as_deref().unwrap().contains("TODO"));
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
                response_format: ResponseFormat::Detailed,
            }),
        )
        .await
        .unwrap();

        let spans = resp.0.spans.expect("detailed format returns spans");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].symbol.as_deref(), Some("beta"));
        assert_eq!(spans[0].line_range, [1, 2]);
    }
}
