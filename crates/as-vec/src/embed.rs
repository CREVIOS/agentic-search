//! Embedding wrapper around fastembed-rs.

use as_core::{Error, Result};
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

pub struct Embedder {
    inner: TextEmbedding,
    pub dim: usize,
}

impl Embedder {
    /// Construct with a known model. Default: BGE-small-en-v1.5 (384d).
    pub fn new(model: EmbeddingModel) -> Result<Self> {
        let dim = match model {
            EmbeddingModel::BGESmallENV15 => 384,
            EmbeddingModel::BGEBaseENV15 => 768,
            EmbeddingModel::BGELargeENV15 => 1024,
            EmbeddingModel::AllMiniLML6V2 => 384,
            _ => 384,
        };
        let inner = TextEmbedding::try_new(InitOptions::new(model))
            .map_err(|e| Error::Index(format!("embedder init: {e}")))?;
        Ok(Self { inner, dim })
    }

    pub fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
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
