//! Minimal MCP stdio server. Speaks the Model Context Protocol over
//! line-delimited JSON-RPC 2.0 on stdin/stdout — enough to register with
//! Claude Code, Claude Agent SDK, Cursor, and any other MCP host.
//!
//! Implements:
//!   - `initialize`        → server capabilities + version
//!   - `tools/list`        → enumerate `ls`, `glob`, `read`, `grep`,
//!     `find_symbol`, `search`
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

fn tools_manifest() -> Vec<Value> {
    vec![
        json!({
            "name": "ls",
            "description": "List objects under an S3/GCS/R2/file URI prefix. Optional glob pattern relative to the prefix.",
            "inputSchema": {
                "type": "object",
                "required": ["uri"],
                "properties": {
                    "uri": { "type": "string", "description": "e.g. s3://bucket/path/, file:///abs/path" },
                    "glob": { "type": "string", "description": "Glob relative to the prefix (e.g. **/*.rs)" },
                    "limit": { "type": "integer", "default": 1000 }
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
                    "uri": { "type": "string" },
                    "offset": { "type": "integer" },
                    "length": { "type": "integer" }
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
                    "uri": { "type": "string" },
                    "pattern": { "type": "string" },
                    "case_insensitive": { "type": "boolean", "default": false },
                    "max_hits": { "type": "integer", "default": 1000 },
                    "concurrency": { "type": "integer", "default": 32 },
                    "ast": { "type": "boolean", "default": false }
                }
            }
        }),
        json!({
            "name": "find_symbol",
            "description": "Locate a function/class/method by exact name across a prefix. Always AST-widened.",
            "inputSchema": {
                "type": "object",
                "required": ["uri", "symbol"],
                "properties": {
                    "uri": { "type": "string" },
                    "symbol": { "type": "string" },
                    "max_hits": { "type": "integer", "default": 200 },
                    "concurrency": { "type": "integer", "default": 32 }
                }
            }
        }),
        json!({
            "name": "search",
            "description": "Hybrid search over a prefix. Currently grep+AST; future versions may also fan out to optional vector / web stages.",
            "inputSchema": {
                "type": "object",
                "required": ["uri", "query"],
                "properties": {
                    "uri": { "type": "string" },
                    "query": { "type": "string" },
                    "k": { "type": "integer", "default": 20 }
                }
            }
        }),
        json!({
            "name": "delegate",
            "description": "Run a search-only subagent loop with a wall-time budget. Returns a one-paragraph summary plus compressed citations to spans. Use this when a lead agent wants ONE call that answers a 'find / explain / locate' question without consuming its own context.",
            "inputSchema": {
                "type": "object",
                "required": ["uri", "query"],
                "properties": {
                    "uri": { "type": "string" },
                    "query": { "type": "string" },
                    "k": { "type": "integer", "default": 20 },
                    "budget_ms": { "type": "integer", "default": 5000 }
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
    // MCP `tools/call` shape: content blocks. We return a single JSON
    // content block so hosts can structurally parse it.
    Ok(json!({
        "content": [
            { "type": "text", "text": serde_json::to_string_pretty(&result_value)? }
        ],
        "isError": false,
        "structuredContent": result_value
    }))
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
