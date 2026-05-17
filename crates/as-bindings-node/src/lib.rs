//! Node bindings. v0 surfaces a stub; M7 wires the planner.

#![deny(clippy::all)]

use napi_derive::napi;

#[napi]
pub fn search(query: String) -> String {
    format!("agentic-search: stub for query={query:?}")
}
