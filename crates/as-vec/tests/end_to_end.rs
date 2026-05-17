//! End-to-end: build a centroid index over a tiny markdown corpus, then
//! query it. Verifies the on-disk format, the build pipeline and the
//! two-roundtrip query path together.
//!
//! Skipped unless `AS_E2E=1` is set, because the first run downloads
//! the BGE-small ONNX model (~33 MB) and ort initialisation makes the
//! default `cargo test` runs heavier than they should be.

use as_embed::{Embedder, Model};
use as_vec::build::{build_index, InputDoc};
use as_vec::query::VecIndex;
use tempfile::tempdir;

fn enabled() -> bool {
    std::env::var("AS_E2E").ok().as_deref() == Some("1")
}

#[tokio::test]
async fn build_and_query_centroid_index() {
    if !enabled() {
        eprintln!("AS_E2E=1 not set; skipping end-to-end index test");
        return;
    }
    let dir = tempdir().unwrap();
    let uri = format!("file://{}", dir.path().display());
    let (store, _) = as_store::open(&uri).expect("open store");

    // Tiny corpus: 6 docs, two roughly-distinct topics.
    let docs = vec![
        ("a.md", "Kubernetes pod scheduling and node taints"),
        (
            "b.md",
            "How Kubernetes admission controllers gate workloads",
        ),
        (
            "c.md",
            "Rust async runtime: tokio task scheduling under load",
        ),
        ("d.md", "Why ripgrep is fast on object storage backends"),
        ("e.md", "Postgres MVCC and the cost of long transactions"),
        (
            "f.md",
            "Designing an S3-native vector index with centroid clusters",
        ),
    ];
    let inputs: Vec<InputDoc> = docs
        .iter()
        .map(|(uri, text)| InputDoc {
            uri: (*uri).to_string(),
            byte_range: [0, text.len() as u64],
            text: (*text).to_string(),
        })
        .collect();

    let manifest = build_index(
        store.clone(),
        "index",
        inputs,
        Model::BgeSmallEnV15,
        Some(2),
        5,
    )
    .await
    .expect("build index");
    assert_eq!(manifest.num_docs, 6);
    assert_eq!(manifest.k, 2);
    assert_eq!(manifest.dim, 384);

    let idx = VecIndex::open(store, "index").await.expect("open index");
    let embedder = Embedder::new(Model::BgeSmallEnV15).expect("embedder");

    let hits = idx
        .query_text(&embedder, "vector search on s3 with cluster shards", 3, 2)
        .await
        .expect("query");
    assert!(!hits.is_empty());
    let top = &hits[0];
    assert_eq!(
        top.doc.uri, "f.md",
        "top hit should be the centroid-cluster doc, got {:?}",
        hits
    );
}
