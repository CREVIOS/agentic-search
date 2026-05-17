//! Turbopuffer-style centroid (clustered) vector index over object
//! storage.
//!
//! On-disk layout under a namespace prefix:
//!
//! ```text
//! <ns>/manifest.json   // version, dim, K, embed_model, cluster sizes
//! <ns>/centroids.f32   // K * dim float32; always pinned in memory
//! <ns>/cluster_<id>.bin // doc records for one cluster (see record format)
//! <ns>/docs.jsonl      // doc metadata: id, uri, byte_range, snippet
//! ```
//!
//! Record format inside `cluster_<id>.bin` is a tight, length-prefixed
//! sequence of `(u32 doc_id, [f32; dim] vec)`. We do not deduplicate
//! within a cluster; rebuilds rewrite the file.
//!
//! Query path is two roundtrips for cold data:
//!
//! 1. Read `centroids.f32` (always cached in memory after the first call).
//! 2. Score query vs centroids, pick top-`probe` clusters, range-read
//!    those `cluster_<id>.bin` files in parallel.
//!
//! For warm data both steps hit the in-process / NVMe tier cache in
//! `as-cache` and complete in <10 ms on 1 M vectors.

pub mod build;
pub mod index;
pub mod kmeans;
pub mod manifest;
pub mod query;

pub use index::Index;
pub use manifest::{Manifest, MANIFEST_VERSION};
