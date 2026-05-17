//! Index manifest. Written atomically last; older readers must ignore
//! intermediate state.

use as_embed::Model;
use serde::{Deserialize, Serialize};

pub const MANIFEST_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub dim: usize,
    /// Number of clusters (centroids).
    pub k: usize,
    pub embed_model: Model,
    /// Total number of indexed documents.
    pub num_docs: u64,
    /// Per-cluster sizes, indexed by cluster id (0..k).
    pub cluster_sizes: Vec<u32>,
    /// Cluster filenames, indexed by cluster id (0..k).
    pub cluster_files: Vec<String>,
    /// Centroid filename.
    pub centroids_file: String,
    /// Docs metadata filename.
    pub docs_file: String,
    /// Chunk size used at index time (characters).
    pub chunk_chars: usize,
    /// Chunk overlap (characters).
    pub chunk_overlap: usize,
}

impl Manifest {
    pub fn new(dim: usize, k: usize, embed_model: Model) -> Self {
        Self {
            version: MANIFEST_VERSION,
            dim,
            k,
            embed_model,
            num_docs: 0,
            cluster_sizes: vec![0; k],
            cluster_files: (0..k).map(|i| format!("cluster_{i:05}.bin")).collect(),
            centroids_file: "centroids.f32".into(),
            docs_file: "docs.jsonl".into(),
            chunk_chars: 1200,
            chunk_overlap: 200,
        }
    }
}
