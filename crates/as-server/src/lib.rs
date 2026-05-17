//! HTTP API + MCP server. v0 exposes a stub `/search` endpoint; M5 wires the
//! full planner and an MCP stdio bridge.

use axum::{routing::post, Json, Router};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub k: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub hits: Vec<as_core::Hit>,
}

pub fn router() -> Router {
    Router::new().route("/search", post(search))
}

async fn search(Json(_req): Json<SearchRequest>) -> Json<SearchResponse> {
    Json(SearchResponse { hits: vec![] })
}
