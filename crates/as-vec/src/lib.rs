//! Vector layer: embeddings (via fastembed-rs / ONNX) + HNSW ANN.

use as_core::{Error, Hit, Result};
use hnsw_rs::prelude::{DistCosine, Hnsw};
use std::sync::RwLock;

pub mod embed;

pub struct VectorIndex {
    inner: RwLock<Hnsw<'static, f32, DistCosine>>,
    ids: RwLock<Vec<String>>,
    uris: RwLock<Vec<String>>,
    dim: usize,
}

impl VectorIndex {
    pub fn new(dim: usize, max_nb_connection: usize, ef_construction: usize) -> Self {
        let hnsw = Hnsw::<f32, DistCosine>::new(
            max_nb_connection,
            10_000_000,
            16,
            ef_construction,
            DistCosine {},
        );
        Self {
            inner: RwLock::new(hnsw),
            ids: RwLock::new(Vec::new()),
            uris: RwLock::new(Vec::new()),
            dim,
        }
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    pub fn insert(&self, id: String, uri: String, vec: &[f32]) -> Result<()> {
        if vec.len() != self.dim {
            return Err(Error::Index(format!(
                "dim mismatch: got {}, want {}",
                vec.len(),
                self.dim
            )));
        }
        let mut ids = self.ids.write().unwrap();
        let mut uris = self.uris.write().unwrap();
        let idx = ids.len();
        ids.push(id);
        uris.push(uri);
        self.inner.write().unwrap().insert((vec, idx));
        Ok(())
    }

    pub fn search(&self, q: &[f32], k: usize, ef: usize) -> Result<Vec<Hit>> {
        if q.len() != self.dim {
            return Err(Error::Index(format!(
                "dim mismatch: got {}, want {}",
                q.len(),
                self.dim
            )));
        }
        let ids = self.ids.read().unwrap();
        let uris = self.uris.read().unwrap();
        let raw = self.inner.read().unwrap().search(q, k, ef);
        Ok(raw
            .into_iter()
            .map(|n| Hit {
                id: ids[n.d_id].clone(),
                uri: uris[n.d_id].clone(),
                score: 1.0 - n.distance,
                snippet: None,
                metadata: serde_json::Value::Null,
            })
            .collect())
    }
}
