use crate::WebSearch;
use as_core::{Error, Hit, Result};
use async_trait::async_trait;

pub struct Brave {
    key: String,
    client: reqwest::Client,
}

impl Brave {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl WebSearch for Brave {
    fn name(&self) -> &'static str {
        "brave"
    }
    async fn search(&self, query: &str, k: usize) -> Result<Vec<Hit>> {
        let resp = self
            .client
            .get("https://api.search.brave.com/res/v1/web/search")
            .header("X-Subscription-Token", &self.key)
            .header("Accept", "application/json")
            .query(&[("q", query), ("count", &k.to_string())])
            .send()
            .await
            .map_err(|e| Error::Other(e.to_string()))?;
        let json: serde_json::Value = resp.json().await.map_err(|e| Error::Other(e.to_string()))?;
        let arr = json["web"]["results"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(arr
            .into_iter()
            .enumerate()
            .map(|(i, v)| Hit {
                id: v["url"].as_str().unwrap_or_default().to_string(),
                uri: v["url"].as_str().unwrap_or_default().to_string(),
                score: 1.0 / (i as f32 + 1.0),
                snippet: v["description"].as_str().map(|s| s.to_string()),
                metadata: v,
            })
            .collect())
    }
}
