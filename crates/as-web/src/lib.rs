//! Web search adapters. Pluggable provider trait, ships with Brave + Tavily.

use as_core::{Error, Hit, Result};
use async_trait::async_trait;

#[async_trait]
pub trait WebSearch: Send + Sync {
    async fn search(&self, query: &str, k: usize) -> Result<Vec<Hit>>;
    fn name(&self) -> &'static str;
}

pub mod brave;
pub mod tavily;

pub fn from_env() -> Result<Box<dyn WebSearch>> {
    if let Ok(k) = std::env::var("BRAVE_API_KEY") {
        return Ok(Box::new(brave::Brave::new(k)));
    }
    if let Ok(k) = std::env::var("TAVILY_API_KEY") {
        return Ok(Box::new(tavily::Tavily::new(k)));
    }
    Err(Error::Config(
        "no web search provider configured (set BRAVE_API_KEY or TAVILY_API_KEY)".into(),
    ))
}
