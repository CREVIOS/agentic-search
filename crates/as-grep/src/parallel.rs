//! Parallel prefix scanner: list a prefix, read each object concurrently,
//! grep its bytes, return all spans across all files.
//!
//! Concurrency is bounded by `ParallelOpts::concurrency` so we do not melt
//! a remote bucket. Large objects are skipped (`max_object_bytes`) on the
//! cold path; `as-cache` will lift this once it is in place.

use crate::{grep_bytes_spans, GrepOpts, Span};
use as_core::{Error, Result};
use as_fs::Fs;
use futures::stream::StreamExt;
use std::sync::Arc;
use tokio::task::JoinSet;

#[derive(Clone, Debug)]
pub struct ParallelOpts {
    pub grep: GrepOpts,
    pub concurrency: usize,
    pub max_object_bytes: u64,
    /// Stop scanning the prefix once this many spans have been collected.
    pub max_total_spans: Option<usize>,
}

impl Default for ParallelOpts {
    fn default() -> Self {
        Self {
            grep: GrepOpts::default(),
            concurrency: 32,
            max_object_bytes: 64 * 1024 * 1024,
            max_total_spans: None,
        }
    }
}

pub struct ParallelGrep {
    fs: Arc<Fs>,
}

impl ParallelGrep {
    pub fn new(fs: Arc<Fs>) -> Self {
        Self { fs }
    }

    /// Walk `prefix`, fetch each object in parallel, regex-grep, return
    /// the union of spans across all files.
    pub async fn scan_prefix(
        &self,
        prefix: &str,
        pattern: &str,
        opts: &ParallelOpts,
    ) -> Result<Vec<Span>> {
        let mut listing = self.fs.list(prefix);
        // Using `JoinSet` so that when `scan_prefix` returns early (cap
        // reached, error, planner dropped the future) every spawned task
        // is aborted automatically. Previously we used `FuturesUnordered`
        // over raw `JoinHandle`s which kept reading objects after we'd
        // moved on — wasted S3 bytes and tokio worker time.
        let mut in_flight: JoinSet<Result<Vec<Span>>> = JoinSet::new();
        let mut spans_all: Vec<Span> = Vec::new();
        let cap = opts.max_total_spans.unwrap_or(usize::MAX);
        let concurrency = opts.concurrency.max(1);

        async fn drain(
            in_flight: &mut JoinSet<Result<Vec<Span>>>,
            spans_all: &mut Vec<Span>,
            cap: usize,
            until: usize,
        ) -> Result<bool> {
            while in_flight.len() > until {
                if let Some(joined) = in_flight.join_next().await {
                    let spans = joined.map_err(|e| Error::Other(format!("join: {e}")))??;
                    spans_all.extend(spans);
                    if spans_all.len() >= cap {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        }

        while let Some(meta_res) = listing.next().await {
            let meta = match meta_res {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!(error = %e, "list error, continuing");
                    continue;
                }
            };
            if meta.size > opts.max_object_bytes {
                continue;
            }

            let fs = self.fs.clone();
            let pattern = pattern.to_string();
            let grep_opts = opts.grep.clone();
            let key = meta.key.clone();

            in_flight.spawn(async move {
                let bytes = fs.read_fresh(&meta).await?;
                grep_bytes_spans(&key, &bytes, &pattern, &grep_opts)
            });

            if drain(&mut in_flight, &mut spans_all, cap, concurrency).await? {
                in_flight.abort_all();
                return Ok(sort_truncate(spans_all, cap));
            }
        }

        drain(&mut in_flight, &mut spans_all, cap, 0).await?;
        Ok(sort_truncate(spans_all, cap))
    }
}

fn sort_truncate(mut spans: Vec<Span>, cap: usize) -> Vec<Span> {
    spans.sort_by(|a, b| {
        a.uri
            .cmp(&b.uri)
            .then(a.line_range[0].cmp(&b.line_range[0]))
            .then(a.byte_range.start.cmp(&b.byte_range.start))
    });
    if spans.len() > cap {
        spans.truncate(cap);
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use tempfile::tempdir;

    #[tokio::test]
    async fn scans_a_prefix_in_parallel() {
        let dir = tempdir().unwrap();
        let uri = format!("file://{}", dir.path().display());
        let (store, _) = as_store::open(&uri).unwrap();
        let fs = Arc::new(Fs::new(store));

        for i in 0..16 {
            let payload = if i % 3 == 0 {
                format!("hello\nfound TODO {i}\nworld\n")
            } else {
                format!("hello\nworld {i}\n")
            };
            fs.write(&format!("data/file_{i:02}.txt"), Bytes::from(payload))
                .await
                .unwrap();
        }

        let pg = ParallelGrep::new(fs.clone());
        let spans = pg
            .scan_prefix("data", "TODO", &ParallelOpts::default())
            .await
            .unwrap();
        assert_eq!(spans.len(), 6); // 0, 3, 6, 9, 12, 15
        for s in &spans {
            assert!(s.snippet.as_deref().unwrap().contains("TODO"));
        }
    }
}
