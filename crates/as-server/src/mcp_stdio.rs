//! Minimal MCP stdio server. Speaks the Model Context Protocol over
//! line-delimited JSON-RPC 2.0 on stdin/stdout — enough to register with
//! Claude Code, Claude Agent SDK, Cursor, and any other MCP host.
//!
//! Implements:
//!   - `initialize`        → server capabilities + version
//!   - `tools/list`        → enumerate `ls`, `read`, `grep`,
//!     `find_symbol`, `search`, `delegate`
//!   - `tools/call`        → dispatch to the same handlers used by REST
//!   - `ping`              → liveness probe

use crate::handlers::{
    self, DelegateRequest, FindRequest, GrepRequest, LsRequest, ReadRequest, SearchRequest,
};
use crate::AppState;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Deserialize)]
struct RpcRequest {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

// Latest MCP spec version supported.
const PROTOCOL_VERSION: &str = "2025-11-25";

pub async fn run() -> anyhow::Result<()> {
    let state = Arc::new(AppState::default());
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let req: RpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                // Parse error → JSON-RPC requires id = null in this case.
                let resp = RpcResponse {
                    jsonrpc: "2.0",
                    id: Value::Null,
                    result: None,
                    error: Some(RpcError {
                        code: -32700,
                        message: format!("parse error: {e}"),
                    }),
                };
                write_line(&mut stdout, &resp).await?;
                continue;
            }
        };
        if req.jsonrpc != "2.0" {
            // Only respond if it had an id (otherwise it was a notification).
            if req.id.is_some() {
                let resp = error_resp(req.id.clone(), -32600, "jsonrpc must be 2.0");
                write_line(&mut stdout, &resp).await?;
            }
            continue;
        }
        // JSON-RPC 2.0: notifications have no `id`. Per the spec, the server
        // MUST NOT send any response for a notification. We still dispatch
        // so internal state can react (e.g. `notifications/initialized`).
        let is_notification = req.id.is_none();
        let id_for_response = req.id.clone().unwrap_or(Value::Null);
        let result = handle(state.clone(), req).await;
        if is_notification {
            // Silently drop errors from notification handlers; clients
            // would never read them anyway.
            continue;
        }
        let resp = match result {
            Ok(v) => RpcResponse {
                jsonrpc: "2.0",
                id: id_for_response,
                result: Some(v),
                error: None,
            },
            Err(e) => {
                // Map specific failure modes to the correct JSON-RPC error
                // codes. Unknown method = -32601; everything else = -32000.
                let msg = e.to_string();
                let code = if msg.starts_with("method not found") {
                    -32601
                } else {
                    -32000
                };
                RpcResponse {
                    jsonrpc: "2.0",
                    id: id_for_response,
                    result: None,
                    error: Some(RpcError { code, message: msg }),
                }
            }
        };
        write_line(&mut stdout, &resp).await?;
    }
    Ok(())
}

async fn handle(state: Arc<AppState>, req: RpcRequest) -> anyhow::Result<Value> {
    match req.method.as_str() {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "serverInfo": { "name": "agentic-search", "version": env!("CARGO_PKG_VERSION") },
            "capabilities": { "tools": { "listChanged": false } }
        })),
        // Notifications: just ack. We never reply because `run` already
        // suppresses responses for messages without `id`.
        "notifications/initialized" | "initialized" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools_manifest() })),
        "tools/call" => tools_call(state, req.params).await,
        other => anyhow::bail!("method not found: {other}"),
    }
}

fn span_schema() -> Value {
    json!({
        "type": "object",
        "required": ["uri", "byte_range", "line_range", "kind", "score"],
        "properties": {
            "uri":        { "type": "string" },
            "byte_range": {
                "type": "object",
                "required": ["start", "end"],
                "properties": {
                    "start": { "type": "integer", "minimum": 0 },
                    "end":   { "type": "integer", "minimum": 0 }
                }
            },
            "line_range": {
                "type": "array",
                "minItems": 2, "maxItems": 2,
                "items": { "type": "integer", "minimum": 1 }
            },
            "symbol":   { "type": ["string", "null"] },
            "kind":     { "type": "string",
                          "enum": ["line", "block", "function", "method", "class", "module"] },
            "snippet":  { "type": ["string", "null"] },
            "score":    { "type": "number" }
        }
    })
}

fn concise_span_schema() -> Value {
    json!({
        "type": "object",
        "required": ["uri", "line_range", "kind", "snippet"],
        "properties": {
            "uri":        { "type": "string" },
            "line_range": {
                "type": "array",
                "minItems": 2, "maxItems": 2,
                "items": { "type": "integer", "minimum": 1 }
            },
            "kind":     { "type": "string",
                          "enum": ["line", "block", "function", "method", "class", "module"] },
            "symbol":   { "type": ["string", "null"] },
            "snippet":  { "type": "string" }
        }
    })
}

fn spans_response_schema() -> Value {
    json!({
        "type": "object",
        "required": ["format"],
        "properties": {
            "format":  { "type": "string", "enum": ["detailed", "concise", "jsonl"] },
            "spans":   { "type": "array", "items": span_schema() },
            "concise": { "type": "array", "items": concise_span_schema() },
            "jsonl":   { "type": "string",
                         "description": "Newline-delimited JSON spans; one Span per line. Populated when response_format=jsonl." }
        }
    })
}

fn response_format_prop() -> Value {
    json!({
        "type": "string",
        "enum": ["detailed", "concise", "jsonl"],
        "default": "detailed",
        "description": "Output verbosity. `concise` for the lead-agent loop (uri + line_range + 1-line snippet, capped at 160 chars). `jsonl` for stream-parsing inside the tool call."
    })
}

pub fn tools_manifest() -> Vec<Value> {
    vec![
        json!({
            "name": "ls",
            "description": "List objects under an S3/GCS/R2/file URI prefix. Optional glob pattern relative to the prefix.",
            "inputSchema": {
                "type": "object",
                "required": ["uri"],
                "properties": {
                    "uri":   { "type": "string", "description": "e.g. s3://bucket/path/, file:///abs/path" },
                    "glob":  { "type": "string", "description": "Glob relative to the prefix (e.g. **/*.rs)" },
                    "limit": { "type": "integer", "default": 1000 }
                }
            },
            "outputSchema": {
                "type": "object",
                "required": ["entries"],
                "properties": {
                    "entries": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["key", "size"],
                            "properties": {
                                "key":  { "type": "string" },
                                "size": { "type": "integer", "minimum": 0 }
                            }
                        }
                    }
                }
            }
        }),
        json!({
            "name": "read",
            "description": "Read an object (optionally a byte range). Returns text when UTF-8, byte count always.",
            "inputSchema": {
                "type": "object",
                "required": ["uri"],
                "properties": {
                    "uri":    { "type": "string" },
                    "offset": { "type": "integer", "minimum": 0 },
                    "length": { "type": "integer", "minimum": 0 }
                }
            },
            "outputSchema": {
                "type": "object",
                "required": ["uri", "bytes"],
                "properties": {
                    "uri":   { "type": "string" },
                    "bytes": { "type": "integer", "minimum": 0 },
                    "text":  { "type": ["string", "null"] }
                }
            }
        }),
        json!({
            "name": "grep",
            "description": "Parallel ripgrep over an object-store prefix. Returns spans aligned to byte ranges. With ast=true, each match is widened to its enclosing function/class/method via tree-sitter.",
            "inputSchema": {
                "type": "object",
                "required": ["uri", "pattern"],
                "properties": {
                    "uri":              { "type": "string" },
                    "pattern":          { "type": "string" },
                    "case_insensitive": { "type": "boolean", "default": false },
                    "max_hits":         { "type": "integer", "default": 1000 },
                    "concurrency":      { "type": "integer", "default": 32 },
                    "ast":              { "type": "boolean", "default": false },
                    "response_format":  response_format_prop()
                }
            },
            "outputSchema": spans_response_schema()
        }),
        json!({
            "name": "find_symbol",
            "description": "Locate a function/class/method by exact name across a prefix. Tree-sitter widens each grep hit and the planner drops spans whose AST symbol name does not match.",
            "inputSchema": {
                "type": "object",
                "required": ["uri", "symbol"],
                "properties": {
                    "uri":             { "type": "string" },
                    "symbol":          { "type": "string" },
                    "max_hits":        { "type": "integer", "default": 200 },
                    "concurrency":     { "type": "integer", "default": 32 },
                    "response_format": response_format_prop()
                }
            },
            "outputSchema": spans_response_schema()
        }),
        json!({
            "name": "search",
            "description": "Hybrid search over a prefix. Runs the planner (parallel grep + AST widening; vector stage if an index exists at .agentic-search/index/<namespace>/).",
            "inputSchema": {
                "type": "object",
                "required": ["uri", "query"],
                "properties": {
                    "uri":             { "type": "string" },
                    "query":           { "type": "string" },
                    "k":               { "type": "integer", "default": 20 },
                    "response_format": response_format_prop()
                }
            },
            "outputSchema": spans_response_schema()
        }),
        json!({
            "name": "delegate",
            "description": "Run a search-only subagent loop with a wall-time budget. Returns a one-paragraph summary plus compressed citations to spans. Use when a lead agent wants ONE call that answers a 'find / explain / locate' question without spending its own context on a loop.",
            "inputSchema": {
                "type": "object",
                "required": ["uri", "query"],
                "properties": {
                    "uri":       { "type": "string" },
                    "query":     { "type": "string" },
                    "k":         { "type": "integer", "default": 20 },
                    "budget_ms": { "type": "integer", "default": 5000 }
                }
            },
            "outputSchema": {
                "type": "object",
                "required": ["summary", "findings", "stats"],
                "properties": {
                    "summary":  { "type": "string" },
                    "findings": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "required": ["uri", "line_range", "byte_range", "kind", "snippet"],
                            "properties": {
                                "uri":        { "type": "string" },
                                "line_range": { "type": "array", "minItems": 2, "maxItems": 2,
                                                "items": { "type": "integer", "minimum": 1 } },
                                "byte_range": { "type": "array", "minItems": 2, "maxItems": 2,
                                                "items": { "type": "integer", "minimum": 0 } },
                                "symbol":     { "type": ["string", "null"] },
                                "kind":       { "type": "string",
                                                "enum": ["line", "block", "function", "method", "class", "module"] },
                                "snippet":    { "type": "string" }
                            }
                        }
                    },
                    "stats": { "type": "object" }
                }
            }
        }),
    ]
}

async fn tools_call(state: Arc<AppState>, params: Value) -> anyhow::Result<Value> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing tool name"))?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let result_value: Value = match name {
        "ls" => {
            let req: LsRequest = serde_json::from_value(args)?;
            let resp = handlers::ls(State(state), axum::Json(req))
                .await
                .map_err(|e| anyhow::anyhow!("{:?}", e))?;
            serde_json::to_value(resp.0)?
        }
        "read" => {
            let req: ReadRequest = serde_json::from_value(args)?;
            let resp = handlers::read(State(state), axum::Json(req))
                .await
                .map_err(|e| anyhow::anyhow!("{:?}", e))?;
            serde_json::to_value(resp.0)?
        }
        "grep" => {
            let req: GrepRequest = serde_json::from_value(args)?;
            let resp = handlers::grep(State(state), axum::Json(req))
                .await
                .map_err(|e| anyhow::anyhow!("{:?}", e))?;
            serde_json::to_value(resp.0)?
        }
        "find_symbol" => {
            let req: FindRequest = serde_json::from_value(args)?;
            let resp = handlers::find(State(state), axum::Json(req))
                .await
                .map_err(|e| anyhow::anyhow!("{:?}", e))?;
            serde_json::to_value(resp.0)?
        }
        "search" => {
            let req: SearchRequest = serde_json::from_value(args)?;
            let resp = handlers::search(State(state), axum::Json(req))
                .await
                .map_err(|e| anyhow::anyhow!("{:?}", e))?;
            serde_json::to_value(resp.0)?
        }
        "delegate" => {
            let req: DelegateRequest = serde_json::from_value(args)?;
            let resp = handlers::delegate(State(state), axum::Json(req))
                .await
                .map_err(|e| anyhow::anyhow!("{:?}", e))?;
            serde_json::to_value(resp.0)?
        }
        other => anyhow::bail!("unknown tool: {other}"),
    };
    // MCP `tools/call` returns `content` blocks for legacy clients
    // plus `structuredContent` for schema-aware clients. We used to
    // pretty-print the whole structured value into a text block,
    // which doubled the payload (and the prettyprint expanded it
    // further). Mirror a short one-line summary instead; clients that
    // want the full result already have `structuredContent`.
    let summary = mcp_text_summary(&result_value);
    Ok(json!({
        "content": [ { "type": "text", "text": summary } ],
        "isError": false,
        "structuredContent": result_value
    }))
}

/// One-line summary suitable as the MCP text mirror. Keeps the
/// response small even when `structuredContent` is large.
fn mcp_text_summary(value: &Value) -> String {
    // Common shapes we emit: `{ spans: [...] }`, `{ entries: [...] }`,
    // `{ findings: [...], summary: "..." }`, `{ format: "concise", ... }`.
    if let Some(s) = value.get("summary").and_then(Value::as_str) {
        return s.to_string();
    }
    if let Some(arr) = value.get("spans").and_then(Value::as_array) {
        return format!("{} span(s)", arr.len());
    }
    if let Some(arr) = value.get("concise").and_then(Value::as_array) {
        return format!("{} span(s) [concise]", arr.len());
    }
    if let Some(arr) = value.get("entries").and_then(Value::as_array) {
        return format!(
            "{} entr{}",
            arr.len(),
            if arr.len() == 1 { "y" } else { "ies" }
        );
    }
    if let Some(bytes) = value.get("bytes").and_then(Value::as_u64) {
        return format!("{bytes} bytes");
    }
    "ok".to_string()
}

fn error_resp(id: Option<Value>, code: i32, msg: &str) -> RpcResponse {
    RpcResponse {
        jsonrpc: "2.0",
        id: id.unwrap_or(Value::Null),
        result: None,
        error: Some(RpcError {
            code,
            message: msg.into(),
        }),
    }
}

async fn write_line(out: &mut tokio::io::Stdout, resp: &RpcResponse) -> anyhow::Result<()> {
    let s = serde_json::to_string(resp)?;
    out.write_all(s.as_bytes()).await?;
    out.write_all(b"\n").await?;
    out.flush().await?;
    Ok(())
}
