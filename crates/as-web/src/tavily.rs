use crate::WebSearch;
use as_core::{Error, Hit, Result};
use async_trait::async_trait;

pub struct Tavily {
    key: String,
    client: reqwest::Client,
}

impl Tavily {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl WebSearch for Tavily {
    fn name(&self) -> &'static str {
        "tavily"
    }
    async fn search(&self, query: &str, k: usize) -> Result<Vec<Hit>> {
        let body = serde_json::json!({
            "api_key": self.key,
            "query": query,
            "max_results": k,
            "search_depth": "advanced",
            "include_answer": false,
        });
        let resp = self
            .client
            .post("https://api.tavily.com/search")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Other(e.to_string()))?;
        let json: serde_json::Value = resp.json().await.map_err(|e| Error::Other(e.to_string()))?;
        let arr = json["results"].as_array().cloned().unwrap_or_default();
        Ok(arr
            .into_iter()
            .map(|v| Hit {
                id: v["url"].as_str().unwrap_or_default().to_string(),
                uri: v["url"].as_str().unwrap_or_default().to_string(),
                score: v["score"].as_f64().unwrap_or(0.0) as f32,
                snippet: v["content"].as_str().map(|s| s.to_string()),
                metadata: v,
            })
            .collect())
    }
}
