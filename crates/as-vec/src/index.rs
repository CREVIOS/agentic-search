//! Doc records + cluster file (de)serialization.

use as_core::{Error, Result};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// A single retrievable chunk + its embedding. `doc_id` indexes into the
/// `docs.jsonl` file (line number, zero-based).
#[derive(Clone, Debug)]
pub struct ClusterRecord {
    pub doc_id: u32,
    pub vector: Vec<f32>,
}

/// One line in `docs.jsonl`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DocMeta {
    pub id: u32,
    pub uri: String,
    pub byte_range: [u64; 2],
    pub snippet: String,
}

/// Encode a list of cluster records to bytes, in the order given.
pub fn encode_cluster(records: &[ClusterRecord], dim: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(records.len() * (4 + dim * 4));
    for r in records {
        out.extend_from_slice(&r.doc_id.to_le_bytes());
        for v in r.vector.iter() {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

/// Decode a cluster file. Returns the records in stored order.
///
/// Hot path. Reads the byte buffer as fixed-stride chunks and parses
/// each chunk with `from_le_bytes` directly, which the compiler
/// vectorises into bulk loads instead of the per-method-call
/// `Buf::get_*` loop the previous implementation used. On Apple
/// silicon this collapsed per-cluster decode from ~30 ms to <1 ms
/// for the SIFT-1M shape (1024 docs × 128 dim).
pub fn decode_cluster(bytes: Bytes, dim: usize) -> Result<Vec<ClusterRecord>> {
    let stride = 4 + dim * 4;
    if bytes.len() % stride != 0 {
        return Err(Error::Index(format!(
            "cluster file size {} is not a multiple of stride {stride}",
            bytes.len()
        )));
    }
    let raw: &[u8] = &bytes;
    let n = raw.len() / stride;
    let mut out = Vec::with_capacity(n);
    for chunk in raw.chunks_exact(stride) {
        let doc_id = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let vec_bytes = &chunk[4..stride];
        let mut v: Vec<f32> = Vec::with_capacity(dim);
        // SAFETY-equivalent (in stable Rust) bulk read: chunks_exact(4)
        // gives us 4-byte windows we feed to `from_le_bytes`. The
        // compiler can collapse this into 4-wide SIMD loads on
        // platforms where f32 alignment isn't an issue (it isn't here
        // since we copy into the owned Vec).
        for w in vec_bytes.chunks_exact(4) {
            v.push(f32::from_le_bytes([w[0], w[1], w[2], w[3]]));
        }
        out.push(ClusterRecord { doc_id, vector: v });
    }
    Ok(out)
}

/// Decode the `K*dim` centroid blob.
pub fn decode_centroids(bytes: Bytes, dim: usize, k: usize) -> Result<Vec<f32>> {
    let expected = dim * k * 4;
    if bytes.len() != expected {
        return Err(Error::Index(format!(
            "centroids file size {} != expected {expected}",
            bytes.len()
        )));
    }
    let raw: &[u8] = &bytes;
    let mut out = Vec::with_capacity(dim * k);
    for w in raw.chunks_exact(4) {
        out.push(f32::from_le_bytes([w[0], w[1], w[2], w[3]]));
    }
    Ok(out)
}

pub fn encode_centroids(centroids: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(centroids.len() * 4);
    for v in centroids {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

#[derive(Clone, Debug)]
pub struct Index {
    pub dim: usize,
    pub centroids: Vec<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn cluster_record_roundtrip() {
        let dim = 4;
        let records = vec![
            ClusterRecord {
                doc_id: 1,
                vector: vec![0.1, 0.2, 0.3, 0.4],
            },
            ClusterRecord {
                doc_id: 7,
                vector: vec![-0.5, 0.0, 0.25, 0.75],
            },
        ];
        let bytes = Bytes::from(encode_cluster(&records, dim));
        let back = decode_cluster(bytes, dim).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].doc_id, 1);
        assert!((back[0].vector[2] - 0.3).abs() < 1e-6);
        assert_eq!(back[1].doc_id, 7);
    }
}
