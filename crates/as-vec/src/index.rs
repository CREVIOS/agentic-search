//! Doc records + cluster file (de)serialization.

use as_core::{Error, Result};
use bytes::{Buf, Bytes};
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
pub fn decode_cluster(mut bytes: Bytes, dim: usize) -> Result<Vec<ClusterRecord>> {
    let stride = 4 + dim * 4;
    if bytes.len() % stride != 0 {
        return Err(Error::Index(format!(
            "cluster file size {} is not a multiple of stride {stride}",
            bytes.len()
        )));
    }
    let n = bytes.len() / stride;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let doc_id = bytes.get_u32_le();
        let mut v = Vec::with_capacity(dim);
        for _ in 0..dim {
            v.push(bytes.get_f32_le());
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
    let mut buf = bytes;
    let mut out = Vec::with_capacity(dim * k);
    for _ in 0..(dim * k) {
        out.push(buf.get_f32_le());
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
