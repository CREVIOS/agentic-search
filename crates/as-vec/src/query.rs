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
use lru::LruCache;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::num::NonZeroUsize;
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VecHit {
    pub doc: DocMeta,
    pub score: f32,
}

/// Process-local cap on how many cluster files we keep decoded in
/// memory. Tunable later via `VecIndex::with_capacity`; the default
/// covers all-clusters-warm for the index sizes we ship with.
pub const DEFAULT_CLUSTER_CACHE: usize = 4096;

pub struct VecIndex {
    pub store: ArcStore,
    pub prefix: String,
    pub manifest: Manifest,
    centroids: Vec<f32>,
    docs: Vec<DocMeta>,
    /// LRU of decoded cluster records keyed by cluster id. Cold
    /// clusters get dropped before memory grows unbounded.
    cluster_cache: parking_lot::Mutex<LruCache<u32, Arc<Vec<crate::index::ClusterRecord>>>>,
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
            cluster_cache: parking_lot::Mutex::new(LruCache::new(
                NonZeroUsize::new(DEFAULT_CLUSTER_CACHE).unwrap(),
            )),
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

        // Score each cluster *as it arrives* and drop our strong ref
        // before pulling the next one in. The cluster cache still
        // holds its own `Arc`, so a hot cluster stays warm; this loop
        // does not pile up `probe` decoded clusters in memory before
        // the first record is scored. For probe=64, 5000 docs each,
        // dim=384, that's the difference between ~470 MB resident and
        // ~7 MB.
        let dim = self.manifest.dim;
        let k = k.max(1);
        let mut heap: BinaryHeap<Reverse<HeapEntry>> = BinaryHeap::with_capacity(k + 1);
        while let Some(r) = futs.next().await {
            let (cid, recs) = r?;
            self.cluster_cache.lock().put(cid, recs.clone());
            for rec in recs.iter() {
                let s: f32 = query
                    .iter()
                    .take(dim)
                    .zip(rec.vector.iter())
                    .map(|(a, b)| a * b)
                    .sum();
                let entry = HeapEntry {
                    score: s,
                    doc_id: rec.doc_id,
                };
                if heap.len() < k {
                    heap.push(Reverse(entry));
                } else if let Some(Reverse(min)) = heap.peek() {
                    if entry.score.total_cmp(&min.score) == std::cmp::Ordering::Greater {
                        heap.pop();
                        heap.push(Reverse(entry));
                    }
                }
            }
            // `recs` drops here; if the cache evicted it, memory is
            // reclaimed before we go fetch the next cluster.
            drop(recs);
        }
        // BinaryHeap order is not sorted; drain into a vec and sort
        // *that* descending. Sorting `k` items, not all `N`.
        let mut top: Vec<HeapEntry> = heap.into_iter().map(|Reverse(e)| e).collect();
        top.sort_by(|a, b| b.score.total_cmp(&a.score));

        Ok(top
            .into_iter()
            .filter_map(|e| {
                self.docs.get(e.doc_id as usize).map(|d| VecHit {
                    doc: d.clone(),
                    score: e.score,
                })
            })
            .collect())
    }
}

#[derive(Clone, Copy)]
struct HeapEntry {
    score: f32,
    doc_id: u32,
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.score.total_cmp(&other.score) == std::cmp::Ordering::Equal && self.doc_id == other.doc_id
    }
}
impl Eq for HeapEntry {}
impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Order by score using IEEE total_cmp so NaN is well-defined,
        // then break ties on doc_id so equal-score entries are stable.
        self.score
            .total_cmp(&other.score)
            .then_with(|| self.doc_id.cmp(&other.doc_id))
    }
}
