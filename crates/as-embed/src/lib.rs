//! Embedding wrapper. Backed by fastembed-rs (ONNX runtime) so we run
//! locally with no network call after the first model download.
//!
//! Default model: BGE-small-en-v1.5 (384 dim). Small enough that a
//! laptop CPU keeps up with the embedding stream produced by `as-vec
//! index`, but strong enough to clear current public retrieval evals.

use as_core::{Error, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Model {
    #[default]
    BgeSmallEnV15,
    BgeBaseEnV15,
    AllMiniLmL6V2,
}

impl Model {
    pub fn dim(&self) -> usize {
        match self {
            Model::BgeSmallEnV15 => 384,
            Model::BgeBaseEnV15 => 768,
            Model::AllMiniLmL6V2 => 384,
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            Model::BgeSmallEnV15 => "bge-small-en-v1.5",
            Model::BgeBaseEnV15 => "bge-base-en-v1.5",
            Model::AllMiniLmL6V2 => "all-MiniLM-L6-v2",
        }
    }

    fn fastembed(&self) -> EmbeddingModel {
        match self {
            Model::BgeSmallEnV15 => EmbeddingModel::BGESmallENV15,
            Model::BgeBaseEnV15 => EmbeddingModel::BGEBaseENV15,
            Model::AllMiniLmL6V2 => EmbeddingModel::AllMiniLML6V2,
        }
    }
}

pub struct Embedder {
    inner: TextEmbedding,
    model: Model,
}

impl Embedder {
    pub fn new(model: Model) -> Result<Self> {
        let inner = TextEmbedding::try_new(InitOptions::new(model.fastembed()))
            .map_err(|e| Error::Index(format!("embedder init: {e}")))?;
        Ok(Self { inner, model })
    }

    pub fn model(&self) -> &Model {
        &self.model
    }

    pub fn dim(&self) -> usize {
        self.model.dim()
    }

    /// Embed a batch of texts. Returns one f32 vector per input.
    pub fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(vec![]);
        }
        self.inner
            .embed(texts, None)
            .map_err(|e| Error::Index(format!("embed: {e}")))
    }

    pub fn embed_one(&self, text: &str) -> Result<Vec<f32>> {
        let mut v = self.embed(vec![text.to_string()])?;
        v.pop()
            .ok_or_else(|| Error::Index("empty embed result".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dim_matches_model() {
        assert_eq!(Model::BgeSmallEnV15.dim(), 384);
        assert_eq!(Model::BgeBaseEnV15.dim(), 768);
    }
}
