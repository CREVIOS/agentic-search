//! Two-roundtrip query path: load centroids → top-N clusters →
//! fetch those cluster files → score by cosine → return top-k spans.
//!
//! The first roundtrip is the centroid load; the second is a parallel
//! range read of the top-N cluster files. Both reads pass through
//! whatever cache layer the caller attached to the `Store` (typically
//! `as_cache::Tiered`).

use crate::index::{decode_centroids, decode_cluster, DocMeta};
use crate::kmeans::{normalize, top_centroids};
use crate::manifest::Manifest;
use as_core::{Error, Result};
use as_embed::Embedder;
use as_store::ArcStore;
use futures::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VecHit {
    pub doc: DocMeta,
    pub score: f32,
}

pub struct VecIndex {
    pub store: ArcStore,
    pub prefix: String,
    pub manifest: Manifest,
    centroids: Vec<f32>,
    docs: Vec<DocMeta>,
    /// Cluster file bytes pinned in memory after the first probe.
    /// Bounded indirectly by `as-cache` on the store path; this map
    /// is a hot working set.
    cluster_cache: parking_lot::Mutex<HashMap<u32, Arc<Vec<crate::index::ClusterRecord>>>>,
}

impl VecIndex {
    pub async fn open(store: ArcStore, prefix: &str) -> Result<Self> {
        let prefix = prefix.trim_end_matches('/').to_string();
        let key = |name: &str| -> String {
            if prefix.is_empty() {
                name.to_string()
            } else {
                format!("{prefix}/{name}")
            }
        };

        let m_bytes = store.get(&key("manifest.json")).await?;
        let manifest: Manifest = serde_json::from_slice(&m_bytes)
            .map_err(|e| Error::Index(format!("bad manifest: {e}")))?;

        let centroid_bytes = store.get(&key(&manifest.centroids_file)).await?;
        let centroids = decode_centroids(centroid_bytes, manifest.dim, manifest.k)?;

        let docs_bytes = store.get(&key(&manifest.docs_file)).await?;
        let docs_str = std::str::from_utf8(&docs_bytes)
            .map_err(|e| Error::Index(format!("docs.jsonl utf8: {e}")))?;
        let mut docs: Vec<DocMeta> = Vec::with_capacity(manifest.num_docs as usize);
        for line in docs_str.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let d: DocMeta = serde_json::from_str(line)
                .map_err(|e| Error::Index(format!("bad doc meta: {e}")))?;
            docs.push(d);
        }
        Ok(Self {
            store,
            prefix,
            manifest,
            centroids,
            docs,
            cluster_cache: parking_lot::Mutex::new(HashMap::new()),
        })
    }

    fn key(&self, name: &str) -> String {
        if self.prefix.is_empty() {
            name.to_string()
        } else {
            format!("{}/{name}", self.prefix)
        }
    }

    /// `probe` is the number of clusters to inspect (Turbopuffer-speak).
    /// Bigger probe = better recall, more roundtrips. `k` is the number
    /// of doc hits to return.
    pub async fn query_text(
        &self,
        embedder: &Embedder,
        query: &str,
        k: usize,
        probe: usize,
    ) -> Result<Vec<VecHit>> {
        let mut q = embedder.embed_one(query)?;
        normalize(&mut q);
        self.query_vec(&q, k, probe).await
    }

    pub async fn query_vec(&self, query: &[f32], k: usize, probe: usize) -> Result<Vec<VecHit>> {
        if query.len() != self.manifest.dim {
            return Err(Error::Index(format!(
                "query dim {} != index dim {}",
                query.len(),
                self.manifest.dim
            )));
        }
        let probe = probe.max(1).min(self.manifest.k);

        let top = top_centroids(
            query,
            &self.centroids,
            self.manifest.dim,
            self.manifest.k,
            probe,
        );

        // Fetch the chosen clusters in parallel.
        let mut futs: FuturesUnordered<_> = top
            .iter()
            .map(|(cid, _)| {
                let cid = *cid;
                let key = self.key(&self.manifest.cluster_files[cid as usize]);
                let store = self.store.clone();
                let dim = self.manifest.dim;
                let cached = self.cluster_cache.lock().get(&cid).cloned();
                async move {
                    if let Some(c) = cached {
                        return Ok::<(u32, Arc<Vec<crate::index::ClusterRecord>>), Error>((cid, c));
                    }
                    let bytes = store.get(&key).await?;
                    let recs = decode_cluster(bytes, dim)?;
                    Ok((cid, Arc::new(recs)))
                }
            })
            .collect();

        let mut all_records: Vec<Arc<Vec<crate::index::ClusterRecord>>> = Vec::with_capacity(probe);
        while let Some(r) = futs.next().await {
            let (cid, recs) = r?;
            self.cluster_cache.lock().insert(cid, recs.clone());
            all_records.push(recs);
        }

        // Score all retrieved records.
        let dim = self.manifest.dim;
        let mut scored: Vec<(f32, u32)> = Vec::new();
        for recs in &all_records {
            for r in recs.iter() {
                let mut s: f32 = 0.0;
                for j in 0..dim {
                    s += query[j] * r.vector[j];
                }
                scored.push((s, r.doc_id));
            }
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);

        Ok(scored
            .into_iter()
            .filter_map(|(score, doc_id)| {
                self.docs.get(doc_id as usize).map(|d| VecHit {
                    doc: d.clone(),
                    score,
                })
            })
            .collect())
    }
}
