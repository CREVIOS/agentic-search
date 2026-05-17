//! Agent-facing virtual filesystem layered on `as-store`.
//!
//! Exposes POSIX-flavored ops (list, glob, open, read_at) on top of any
//! `Store`. Designed so agents can treat S3 (or any object store) as their
//! primary filesystem without local sync.

use as_core::{Error, Result};
use as_store::{ArcStore, ObjectMeta};
use bytes::Bytes;
use futures::stream::{BoxStream, StreamExt, TryStreamExt};
use globset::Glob;
use std::ops::Range;

pub struct Fs {
    store: ArcStore,
}

/// Internal state for the streaming manifest unfold. Kept here so the
/// closure inside `list_with_manifest` stays an `async move` and not
/// some borrow-tangle over a tuple.
struct ManifestStreamState {
    iter: as_store::manifest::ManifestIter,
    pending: Option<as_store::manifest::ManifestEntry>,
    yielded: usize,
    expected: usize,
}

impl Fs {
    pub fn new(store: ArcStore) -> Self {
        Self { store }
    }

    pub async fn read(&self, key: &str) -> Result<Bytes> {
        self.store.get(key).await
    }

    pub async fn read_fresh(&self, meta: &ObjectMeta) -> Result<Bytes> {
        self.store.get_fresh(meta).await
    }

    pub async fn read_at(&self, key: &str, range: Range<u64>) -> Result<Bytes> {
        self.store.get_range(key, range).await
    }

    pub async fn write(&self, key: &str, data: Bytes) -> Result<()> {
        self.store.put(key, data).await
    }

    pub async fn stat(&self, key: &str) -> Result<ObjectMeta> {
        self.store.head(key).await
    }

    pub fn list<'a>(&'a self, prefix: &'a str) -> BoxStream<'a, Result<ObjectMeta>> {
        self.store.list(prefix)
    }

    /// List via a pre-built prefix manifest when one is present, falling
    /// back to live `list` otherwise. The manifest is a single GET so
    /// cold-S3 listing collapses from paged `ListObjectsV2` to one
    /// round-trip; we accept a small staleness window in exchange
    /// (refresh via `agentic-search index-manifest`).
    ///
    /// Robustness: a manifest whose body is truncated (gzip decoder
    /// raises an error mid-stream, *or* the iterator yields fewer
    /// entries than the header claimed) is treated as broken. Rather
    /// than returning a partial corpus, we fall through to live
    /// `list` so search never silently misses files because the
    /// manifest upload was interrupted.
    pub async fn list_with_manifest<'a>(
        &'a self,
        prefix: &'a str,
    ) -> Result<BoxStream<'a, Result<ObjectMeta>>> {
        match as_store::manifest::stream_manifest(&*self.store, prefix).await? {
            Some((header, mut iter)) => {
                // Pull one entry up front so a header-only manifest
                // (count > 0 but no body) is detected before the
                // caller commits to using it. We accept this tiny
                // eager read — it's one line out of a gzipped JSONL —
                // because the correctness win is large.
                let first = match iter.next_entry() {
                    Ok(opt) => opt,
                    Err(e) => {
                        tracing::warn!(error = %e, "manifest body unreadable, falling back to live list");
                        return Ok(self.store.list(prefix));
                    }
                };
                let expected = header.count as usize;
                if expected > 0 && first.is_none() {
                    tracing::warn!("manifest body empty despite count={}; live list", expected);
                    return Ok(self.store.list(prefix));
                }
                let state = ManifestStreamState {
                    iter,
                    pending: first,
                    yielded: 0,
                    expected,
                };
                let stream = futures::stream::unfold(state, |mut s| async move {
                    if let Some(p) = s.pending.take() {
                        s.yielded += 1;
                        return Some((Ok(p.to_object_meta()), s));
                    }
                    match s.iter.next_entry() {
                        Ok(Some(entry)) => {
                            s.yielded += 1;
                            Some((Ok(entry.to_object_meta()), s))
                        }
                        Ok(None) => {
                            if s.expected > 0 && s.yielded < s.expected {
                                let err = Error::Other(format!(
                                    "manifest truncated: yielded {} of {} entries",
                                    s.yielded, s.expected
                                ));
                                Some((Err(err), s))
                            } else {
                                None
                            }
                        }
                        Err(e) => Some((Err(e), s)),
                    }
                });
                Ok(stream.boxed())
            }
            None => Ok(self.store.list(prefix)),
        }
    }

    /// List keys under `prefix` matching a glob pattern (relative to prefix).
    ///
    /// The pattern is matched against the *relative* portion of each key,
    /// so `glob("docs", "*.md")` matches `docs/a.md` (the relative tail is
    /// `a.md`). Use `**/*.md` to match files at any depth.
    pub fn glob<'a>(
        &'a self,
        prefix: &'a str,
        pattern: &str,
    ) -> Result<BoxStream<'a, Result<ObjectMeta>>> {
        let glob = Glob::new(pattern)
            .map_err(|e| Error::Config(format!("bad glob: {e}")))?
            .compile_matcher();
        let prefix_owned: String = prefix.trim_end_matches('/').to_string();
        Ok(self
            .list(prefix)
            .try_filter(move |m| {
                let tail = m
                    .key
                    .strip_prefix(&prefix_owned)
                    .unwrap_or(&m.key)
                    .trim_start_matches('/');
                let matches = glob.is_match(tail);
                async move { matches }
            })
            .boxed())
    }
}
