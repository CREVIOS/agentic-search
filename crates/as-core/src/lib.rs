//! Core types, errors, and config shared across agentic-search crates.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("store: {0}")]
    Store(String),
    #[error("index: {0}")]
    Index(String),
    #[error("plan: {0}")]
    Plan(String),
    #[error("config: {0}")]
    Config(String),
    #[error("other: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Doc {
    pub id: String,
    pub uri: String,
    pub text: String,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hit {
    pub id: String,
    pub uri: String,
    pub score: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Query {
    pub text: String,
    #[serde(default)]
    pub k: Option<usize>,
    #[serde(default)]
    pub filter: Option<serde_json::Value>,
    #[serde(default)]
    pub mode: QueryMode,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueryMode {
    #[default]
    Hybrid,
    Lexical,
    Vector,
    Web,
    Grep,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub cache_dir: PathBuf,
    pub default_index: String,
    pub embed_model: String,
    pub reranker: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cache_dir: PathBuf::from(".agentic-search/cache"),
            default_index: "default".into(),
            embed_model: "BAAI/bge-small-en-v1.5".into(),
            reranker: None,
        }
    }
}
