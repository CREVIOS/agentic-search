//! Query planner. Fuses lexical + vector + web with Reciprocal Rank Fusion,
//! then applies an optional cross-encoder reranker on the top-N.

use as_core::{Hit, Result};
use std::collections::HashMap;

/// RRF: score(d) = Σ 1 / (k + rank_i(d)). Default k=60 per the original paper.
pub fn rrf(lists: &[Vec<Hit>], k: usize, top_k: usize) -> Vec<Hit> {
    let k_f = k as f32;
    let mut acc: HashMap<String, (f32, Hit)> = HashMap::new();
    for list in lists {
        for (rank, h) in list.iter().enumerate() {
            let contrib = 1.0 / (k_f + rank as f32 + 1.0);
            acc.entry(h.id.clone())
                .and_modify(|(s, _)| *s += contrib)
                .or_insert_with(|| (contrib, h.clone()));
        }
    }
    let mut out: Vec<(f32, Hit)> = acc.into_values().collect();
    out.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    out.into_iter()
        .take(top_k)
        .map(|(s, mut h)| {
            h.score = s;
            h
        })
        .collect()
}

/// Fuse-and-truncate helper used by planner stages.
pub fn fuse(lists: Vec<Vec<Hit>>, top_k: usize) -> Result<Vec<Hit>> {
    Ok(rrf(&lists, 60, top_k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use as_core::Hit;

    fn h(id: &str) -> Hit {
        Hit {
            id: id.into(),
            uri: id.into(),
            score: 0.0,
            snippet: None,
            metadata: serde_json::Value::Null,
        }
    }

    #[test]
    fn rrf_promotes_documents_in_both_lists() {
        let a = vec![h("x"), h("y"), h("z")];
        let b = vec![h("y"), h("w")];
        let r = rrf(&[a, b], 60, 4);
        // y is the only doc ranked highly in both lists; should win.
        assert_eq!(r[0].id, "y");
    }
}
