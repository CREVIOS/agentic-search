//! HTTP + MCP server. Exposes the agentic-search tool surface to any
//! agent runtime: REST for direct calls, MCP stdio for Claude Code /
//! Claude Agent SDK / MCP-aware clients.

pub mod handlers;
pub mod mcp_stdio;
pub mod state;

use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;

pub use state::AppState;

/// Build an axum router. The state is shared across requests so cache /
/// open stores survive across calls.
pub fn router_with_state(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(handlers::health))
        .route("/ls", post(handlers::ls))
        .route("/read", post(handlers::read))
        .route("/grep", post(handlers::grep))
        .route("/find", post(handlers::find))
        .route("/search", post(handlers::search))
        .route("/delegate", post(handlers::delegate))
        .with_state(state)
}

/// Convenience for `agentic-search serve --bind …` — builds a router with
/// a default `AppState`.
pub fn router() -> Router {
    router_with_state(Arc::new(AppState::default()))
}
