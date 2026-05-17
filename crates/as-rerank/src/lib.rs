//! Cross-encoder rerankers. v0 ships a passthrough; M4 wires bge-reranker via candle.

use as_core::{Hit, Result};
use async_trait::async_trait;

#[async_trait]
pub trait Reranker: Send + Sync {
    async fn rerank(&self, query: &str, hits: Vec<Hit>) -> Result<Vec<Hit>>;
    fn name(&self) -> &'static str;
}

pub struct Passthrough;

#[async_trait]
impl Reranker for Passthrough {
    fn name(&self) -> &'static str {
        "passthrough"
    }
    async fn rerank(&self, _query: &str, hits: Vec<Hit>) -> Result<Vec<Hit>> {
        Ok(hits)
    }
}
