//! Index-build pipeline: text chunking + embedding + k-means + write
//! segments to an object store.
//!
//! Caller pre-extracts `(uri, text)` documents and feeds them in. Text
//! extraction for code/markdown/plain lives outside this crate (kept
//! separate so a later content extractor can be added without touching
//! the index format).

use crate::index::{encode_centroids, encode_cluster, ClusterRecord, DocMeta};
use crate::kmeans::{self, normalize};
use crate::manifest::Manifest;
use as_core::{Error, Result};
use as_embed::{Embedder, Model};
use as_store::ArcStore;
use bytes::Bytes;

#[derive(Clone, Debug)]
pub struct InputDoc {
    pub uri: String,
    pub byte_range: [u64; 2],
    pub text: String,
}

/// Build an index from a list of `(uri, text)` chunks and write its
/// segments to `<store>/<namespace_prefix>/`.
///
/// `k` is the cluster count. A good default is `max(1, sqrt(N))`.
pub async fn build_index(
    store: ArcStore,
    namespace_prefix: &str,
    docs: Vec<InputDoc>,
    embed_model: Model,
    k: Option<usize>,
    kmeans_iters: usize,
) -> Result<Manifest> {
    if docs.is_empty() {
        return Err(Error::Index("build_index: no input docs".into()));
    }
    let embedder = Embedder::new(embed_model.clone())?;
    let dim = embedder.dim();
    let k = k.unwrap_or_else(|| ((docs.len() as f64).sqrt() as usize).max(1));
    let k = k.min(docs.len());

    // Embed in batches; fastembed handles its own batching internally,
    // but we cap to keep peak memory predictable.
    let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(docs.len());
    const BATCH: usize = 64;
    for chunk in docs.chunks(BATCH) {
        let texts: Vec<String> = chunk.iter().map(|d| d.text.clone()).collect();
        let mut embedded = embedder.embed(texts)?;
        for v in embedded.iter_mut() {
            normalize(v);
        }
        vectors.append(&mut embedded);
    }
    if vectors.len() != docs.len() {
        return Err(Error::Index("build_index: embedder output mismatch".into()));
    }

    let (centroids, assignments) = kmeans::train(&vectors, k, kmeans_iters)?;

    // Partition records by cluster id.
    let mut buckets: Vec<Vec<ClusterRecord>> = (0..k).map(|_| Vec::new()).collect();
    for (i, &cid) in assignments.iter().enumerate() {
        buckets[cid as usize].push(ClusterRecord {
            doc_id: i as u32,
            vector: vectors[i].clone(),
        });
    }

    let mut manifest = Manifest::new(dim, k, embed_model);
    manifest.num_docs = docs.len() as u64;
    for (cid, bucket) in buckets.iter().enumerate() {
        manifest.cluster_sizes[cid] = bucket.len() as u32;
    }

    // Write segment files. We write data first, then the manifest last;
    // a reader sees a torn write only if it racing-loads a manifest
    // that points at half-uploaded clusters, which we avoid by writing
    // manifest.json strictly after every other file.
    let key = |name: &str| -> String {
        let pref = namespace_prefix.trim_end_matches('/');
        if pref.is_empty() {
            name.to_string()
        } else {
            format!("{pref}/{name}")
        }
    };

    // Centroids.
    let centroid_bytes = encode_centroids(&centroids);
    store
        .put(&key(&manifest.centroids_file), Bytes::from(centroid_bytes))
        .await?;

    // Cluster files.
    for (cid, bucket) in buckets.iter().enumerate() {
        let bytes = encode_cluster(bucket, dim);
        if bytes.is_empty() {
            continue;
        }
        store
            .put(&key(&manifest.cluster_files[cid]), Bytes::from(bytes))
            .await?;
    }

    // docs.jsonl.
    let mut docs_buf = String::new();
    for (i, d) in docs.iter().enumerate() {
        let snippet: String = d.text.chars().take(280).collect();
        let meta = DocMeta {
            id: i as u32,
            uri: d.uri.clone(),
            byte_range: d.byte_range,
            snippet,
        };
        docs_buf.push_str(&serde_json::to_string(&meta).unwrap());
        docs_buf.push('\n');
    }
    store
        .put(
            &key(&manifest.docs_file),
            Bytes::from(docs_buf.into_bytes()),
        )
        .await?;

    // Manifest last.
    let m_bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    store
        .put(&key("manifest.json"), Bytes::from(m_bytes))
        .await?;

    Ok(manifest)
}

/// Char-based chunker for prose / markdown / plain text. Returns
/// `(start_byte, end_byte, chunk_text)`. Chunks overlap by
/// `chunk_overlap` bytes so retrieval span context is preserved.
pub fn chunk_text(text: &str, chunk_chars: usize, overlap: usize) -> Vec<(u64, u64, String)> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return vec![];
    }
    let stride = chunk_chars.saturating_sub(overlap).max(1);
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < bytes.len() {
        let mut end = (start + chunk_chars).min(bytes.len());
        // Keep chunks UTF-8 aligned.
        while end < bytes.len() && (bytes[end] & 0b1100_0000) == 0b1000_0000 {
            end += 1;
        }
        let s = match std::str::from_utf8(&bytes[start..end]) {
            Ok(s) => s.to_string(),
            Err(_) => {
                start += stride;
                continue;
            }
        };
        out.push((start as u64, end as u64, s));
        if end == bytes.len() {
            break;
        }
        start += stride;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_preserves_coverage() {
        let text = "0123456789".repeat(120); // 1200 chars
        let chunks = chunk_text(&text, 500, 100);
        assert!(chunks.len() >= 3);
        assert_eq!(chunks[0].0, 0);
        // Coverage union should span the whole text.
        let max_end = chunks.iter().map(|(_, e, _)| *e).max().unwrap() as usize;
        assert_eq!(max_end, text.len());
    }
}
