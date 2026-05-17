//! Query planner.
//!
//! Span-aware RRF fusion of multiple stages plus a high-level
//! `Planner::search` entry point that picks which stages to run based on
//! the inputs available (an optional vector index, a corpus prefix to
//! grep, …).

use as_core::Result;
use as_embed::{Embedder, Model};
use as_fs::Fs;
use as_grep::{GrepOpts, ParallelGrep, ParallelOpts, RankSignals, SourceStage, Span, SpanKind};
use as_store::ArcStore;
use as_vec::query::{VecHit, VecIndex};
use std::collections::HashMap;
use std::sync::Arc;

pub mod terms;

/// Reciprocal-rank fusion over multiple ranked span lists.
///
/// score(s) = Σ_i 1 / (k + rank_i(s) + 1)
/// where rank starts at 0 and `k = 60` is the value from the original
/// Cormack-Clarke-Bühler paper.
pub fn rrf(lists: &[Vec<Span>], k: usize, top_k: usize) -> Vec<Span> {
    let k_f = k as f32;
    let mut acc: HashMap<String, (f32, Span)> = HashMap::new();
    for list in lists {
        for (rank, s) in list.iter().enumerate() {
            let contrib = 1.0 / (k_f + rank as f32 + 1.0);
            let key = s.dedup_key();
            acc.entry(key)
                .and_modify(|(score, _)| *score += contrib)
                .or_insert_with(|| (contrib, s.clone()));
        }
    }
    let mut out: Vec<(f32, Span)> = acc.into_values().collect();
    out.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    out.into_iter()
        .take(top_k)
        .map(|(score, mut span)| {
            span.score = score;
            span.source_stage = Some(SourceStage::Fusion);
            span
        })
        .collect()
}

/// Vec hits -> spans, so the planner can fuse them on equal footing
/// with grep spans.
pub fn vec_hits_to_spans(hits: Vec<VecHit>) -> Vec<Span> {
    hits.into_iter()
        .map(|h| Span {
            uri: h.doc.uri.clone(),
            byte_range: h.doc.byte_range[0]..h.doc.byte_range[1],
            line_range: [1, 1],
            kind: SpanKind::Block,
            snippet: Some(h.doc.snippet),
            score: h.score,
            source_stage: Some(SourceStage::Vector),
            rank_signals: Some(RankSignals {
                cosine: Some(h.score),
                ..RankSignals::default()
            }),
            ..Span::default()
        })
        .collect()
}

/// Inputs to a single planner run.
pub struct PlanInputs<'a> {
    pub fs: Arc<Fs>,
    pub grep_prefix: &'a str,
    pub query: &'a str,
    pub k: usize,
    pub grep_max_hits: usize,
    pub grep_concurrency: usize,
    pub vec_index: Option<&'a VecIndex>,
    pub vec_probe: usize,
    pub vec_store: Option<ArcStore>,
    /// Per-stage wall-time budget. A stage that misses its deadline is
    /// dropped with a `dropped: true` entry in `PlanResult::stages`
    /// rather than failing the whole call.
    pub budgets: StageBudgets,
}

#[derive(Clone, Copy, Debug)]
pub struct StageBudgets {
    pub grep: std::time::Duration,
    pub vector: std::time::Duration,
}

impl Default for StageBudgets {
    fn default() -> Self {
        Self {
            grep: std::time::Duration::from_millis(2_500),
            vector: std::time::Duration::from_millis(1_500),
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct StageStat {
    pub stage: &'static str,
    pub ms: u128,
    pub hits: usize,
    /// `true` if the stage was dropped from fusion (timed out or errored).
    pub dropped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct PlanResult {
    pub spans: Vec<Span>,
    pub stages: Vec<StageStat>,
}

pub struct Planner;

impl Planner {
    /// Run the grep stage (parallel ripgrep over the prefix).
    pub async fn grep_stage(
        fs: Arc<Fs>,
        prefix: &str,
        query: &str,
        max_hits: usize,
        concurrency: usize,
    ) -> Result<Vec<Span>> {
        let terms = terms::tokenize(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = terms
            .iter()
            .map(|t| terms::regex_escape(t))
            .collect::<Vec<_>>()
            .join("|");
        let opts = ParallelOpts {
            grep: GrepOpts {
                case_insensitive: true,
                multi_line: false,
                max_hits_per_file: None,
            },
            concurrency,
            max_object_bytes: 64 * 1024 * 1024,
            max_total_spans: Some(max_hits),
        };
        ParallelGrep::new(fs)
            .scan_prefix(prefix, &pattern, &opts)
            .await
    }

    /// Run the vector stage if an index is configured.
    pub async fn vec_stage(
        index: &VecIndex,
        embedder: &Embedder,
        query: &str,
        k: usize,
        probe: usize,
    ) -> Result<Vec<Span>> {
        let hits = index.query_text(embedder, query, k, probe).await?;
        Ok(vec_hits_to_spans(hits))
    }

    /// Run each stage under its own deadline and fuse with RRF.
    /// Stages that miss their deadline (or error out) are dropped from
    /// fusion and reported via `PlanResult::stages` instead of failing
    /// the whole call. The headline `search` wrapper returns just the
    /// span list for back-compat; `search_with_stats` exposes the full
    /// `PlanResult` so callers can surface per-stage telemetry.
    pub async fn search(inputs: PlanInputs<'_>) -> Result<Vec<Span>> {
        Ok(Self::search_with_stats(inputs).await?.spans)
    }

    pub async fn search_with_stats(inputs: PlanInputs<'_>) -> Result<PlanResult> {
        let mut lists: Vec<Vec<Span>> = Vec::new();
        let mut stages: Vec<StageStat> = Vec::new();

        // Stage 1: grep over the prefix. Always runs.
        let grep_started = std::time::Instant::now();
        match tokio::time::timeout(
            inputs.budgets.grep,
            Self::grep_stage(
                inputs.fs.clone(),
                inputs.grep_prefix,
                inputs.query,
                inputs.grep_max_hits,
                inputs.grep_concurrency,
            ),
        )
        .await
        {
            Ok(Ok(spans)) => {
                stages.push(StageStat {
                    stage: "grep",
                    ms: grep_started.elapsed().as_millis(),
                    hits: spans.len(),
                    dropped: spans.is_empty(),
                    error: None,
                });
                if !spans.is_empty() {
                    lists.push(spans);
                }
            }
            Ok(Err(e)) => stages.push(StageStat {
                stage: "grep",
                ms: grep_started.elapsed().as_millis(),
                hits: 0,
                dropped: true,
                error: Some(e.to_string()),
            }),
            Err(_) => stages.push(StageStat {
                stage: "grep",
                ms: grep_started.elapsed().as_millis(),
                hits: 0,
                dropped: true,
                error: Some(format!(
                    "budget {}ms exceeded",
                    inputs.budgets.grep.as_millis()
                )),
            }),
        }

        // Stage 2: vector ANN over the namespace, if available.
        if let Some(index) = inputs.vec_index {
            let vec_started = std::time::Instant::now();
            let embedder_res = Embedder::new(index.manifest.embed_model.clone())
                .or_else(|_| Embedder::new(Model::default()));
            match embedder_res {
                Ok(embedder) => {
                    match tokio::time::timeout(
                        inputs.budgets.vector,
                        Self::vec_stage(
                            index,
                            &embedder,
                            inputs.query,
                            inputs.k * 2,
                            inputs.vec_probe,
                        ),
                    )
                    .await
                    {
                        Ok(Ok(spans)) => {
                            stages.push(StageStat {
                                stage: "vector",
                                ms: vec_started.elapsed().as_millis(),
                                hits: spans.len(),
                                dropped: spans.is_empty(),
                                error: None,
                            });
                            if !spans.is_empty() {
                                lists.push(spans);
                            }
                        }
                        Ok(Err(e)) => stages.push(StageStat {
                            stage: "vector",
                            ms: vec_started.elapsed().as_millis(),
                            hits: 0,
                            dropped: true,
                            error: Some(e.to_string()),
                        }),
                        Err(_) => stages.push(StageStat {
                            stage: "vector",
                            ms: vec_started.elapsed().as_millis(),
                            hits: 0,
                            dropped: true,
                            error: Some(format!(
                                "budget {}ms exceeded",
                                inputs.budgets.vector.as_millis()
                            )),
                        }),
                    }
                }
                Err(e) => stages.push(StageStat {
                    stage: "vector",
                    ms: vec_started.elapsed().as_millis(),
                    hits: 0,
                    dropped: true,
                    error: Some(format!("embedder init: {e}")),
                }),
            }
        }

        Ok(PlanResult {
            spans: rrf(&lists, 60, inputs.k),
            stages,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(uri: &str, line: u32, score: f32) -> Span {
        Span {
            uri: uri.into(),
            byte_range: 0..1,
            line_range: [line, line],
            kind: SpanKind::Line,
            score,
            ..Span::default()
        }
    }

    #[test]
    fn rrf_promotes_documents_in_both_lists() {
        let a = vec![span("x", 1, 0.0), span("y", 1, 0.0), span("z", 1, 0.0)];
        let b = vec![span("y", 1, 0.0), span("w", 1, 0.0)];
        let r = rrf(&[a, b], 60, 4);
        // y is the only doc ranked top-2 in both lists; should win.
        assert_eq!(r[0].uri, "y");
    }

    #[tokio::test]
    async fn budget_drops_slow_grep_stage() {
        use as_store::open;
        use bytes::Bytes;
        use tempfile::tempdir;
        // Sub-millisecond grep budget on a fresh corpus → grep is virtually
        // guaranteed to miss its deadline, but the planner must report a
        // dropped stage rather than fail.
        let dir = tempdir().unwrap();
        let uri = format!("file://{}", dir.path().display());
        let (store, _) = open(&uri).unwrap();
        for i in 0..16 {
            store
                .put(
                    &format!("docs/{i}.md"),
                    Bytes::from(format!("# doc {i} fnord async fn body").into_bytes()),
                )
                .await
                .unwrap();
        }
        let fs = Arc::new(Fs::new(store));
        let result = Planner::search_with_stats(PlanInputs {
            fs,
            grep_prefix: "docs",
            query: "async fn body",
            k: 4,
            grep_max_hits: 32,
            grep_concurrency: 4,
            vec_index: None,
            vec_probe: 1,
            vec_store: None,
            budgets: StageBudgets {
                grep: std::time::Duration::from_nanos(1),
                vector: std::time::Duration::from_secs(1),
            },
        })
        .await
        .unwrap();
        let stage = result
            .stages
            .iter()
            .find(|s| s.stage == "grep")
            .expect("grep stage");
        assert!(stage.dropped, "grep should be dropped under sub-µs budget");
        assert!(stage.error.as_deref().unwrap_or("").contains("budget"));
    }
}
