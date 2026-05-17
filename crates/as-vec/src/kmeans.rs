//! Minimal, deterministic mini-batch k-means. Tuned for the index-build
//! path where the vector count is in the millions and we just need
//! reasonable centroids; we are not chasing kmeans++ quality here.
//!
//! Distance is cosine over L2-normalized vectors, which reduces to dot
//! product. The caller is expected to normalize before calling.

use as_core::{Error, Result};
use rayon::prelude::*;

/// Train K centroids over the given vectors. Returns
/// `(centroids[K*dim], assignments[N])`.
pub fn train(vectors: &[Vec<f32>], k: usize, iters: usize) -> Result<(Vec<f32>, Vec<u32>)> {
    if vectors.is_empty() {
        return Err(Error::Index("kmeans: no vectors".into()));
    }
    let n = vectors.len();
    let dim = vectors[0].len();
    if k == 0 || k > n {
        return Err(Error::Index(format!("kmeans: invalid k={k} for n={n}")));
    }

    // Deterministic init: pick K vectors at evenly-spaced positions. Good
    // enough; full kmeans++ would buy a bit of recall at non-trivial cost.
    let stride = (n / k).max(1);
    let mut centroids: Vec<f32> = Vec::with_capacity(k * dim);
    for i in 0..k {
        let src = &vectors[(i * stride).min(n - 1)];
        if src.len() != dim {
            return Err(Error::Index("kmeans: ragged input".into()));
        }
        centroids.extend_from_slice(src);
    }

    let mut assignments = vec![0u32; n];
    for _ in 0..iters {
        // Assignment step (parallel).
        assignments
            .par_iter_mut()
            .enumerate()
            .for_each(|(i, slot)| {
                *slot = nearest_centroid(&vectors[i], &centroids, dim, k) as u32;
            });
        // Update step. Single-threaded — k is small (~sqrt(n)).
        let mut sums = vec![0f32; k * dim];
        let mut counts = vec![0u32; k];
        for (vec, &cid) in vectors.iter().zip(assignments.iter()) {
            let off = cid as usize * dim;
            for (j, v) in vec.iter().enumerate() {
                sums[off + j] += *v;
            }
            counts[cid as usize] += 1;
        }
        for (c, &cnt) in counts.iter().enumerate() {
            if cnt == 0 {
                continue;
            }
            let off = c * dim;
            for j in 0..dim {
                centroids[off + j] = sums[off + j] / cnt as f32;
            }
            // Re-normalize centroid so cosine == dot afterwards.
            let mut norm: f32 = 0.0;
            for j in 0..dim {
                norm += centroids[off + j] * centroids[off + j];
            }
            let norm = norm.sqrt().max(1e-12);
            for j in 0..dim {
                centroids[off + j] /= norm;
            }
        }
    }
    Ok((centroids, assignments))
}

#[inline]
pub fn nearest_centroid(query: &[f32], centroids: &[f32], dim: usize, k: usize) -> usize {
    let mut best = 0usize;
    let mut best_score = f32::MIN;
    for c in 0..k {
        let off = c * dim;
        let mut s = 0f32;
        for j in 0..dim {
            s += query[j] * centroids[off + j];
        }
        if s > best_score {
            best_score = s;
            best = c;
        }
    }
    best
}

/// Top-N centroids by dot product. Returns (cluster_id, score) sorted desc.
pub fn top_centroids(
    query: &[f32],
    centroids: &[f32],
    dim: usize,
    k: usize,
    n: usize,
) -> Vec<(u32, f32)> {
    let mut scores: Vec<(u32, f32)> = (0..k)
        .map(|c| {
            let off = c * dim;
            let mut s = 0f32;
            for j in 0..dim {
                s += query[j] * centroids[off + j];
            }
            (c as u32, s)
        })
        .collect();
    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scores.truncate(n);
    scores
}

/// L2-normalize a vector in place.
pub fn normalize(v: &mut [f32]) {
    let mut n: f32 = 0.0;
    for x in v.iter() {
        n += x * x;
    }
    let n = n.sqrt().max(1e-12);
    for x in v.iter_mut() {
        *x /= n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separable_clusters_converge() {
        // Two well-separated clusters in 4D.
        let mut vecs: Vec<Vec<f32>> = Vec::new();
        for _ in 0..20 {
            let mut v = vec![1.0, 0.0, 0.0, 0.1];
            normalize(&mut v);
            vecs.push(v);
        }
        for _ in 0..20 {
            let mut v = vec![0.0, 1.0, 0.0, 0.1];
            normalize(&mut v);
            vecs.push(v);
        }
        let (centroids, assignments) = train(&vecs, 2, 8).unwrap();
        // Assignments should partition the two halves into different clusters.
        let first_cid = assignments[0];
        let second_cid = assignments[20];
        assert_ne!(first_cid, second_cid);
        // Centroids should be unit-ish.
        let n0: f32 = centroids[0..4].iter().map(|x| x * x).sum();
        assert!((n0 - 1.0).abs() < 0.01);
    }
}
