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
    pub async fn list_with_manifest<'a>(
        &'a self,
        prefix: &'a str,
    ) -> Result<BoxStream<'a, Result<ObjectMeta>>> {
        // Stream the manifest entry-by-entry so we don't materialize a
        // Vec<ManifestEntry> for million-doc prefixes.
        if let Some((_, iter)) = as_store::manifest::stream_manifest(&*self.store, prefix).await? {
            let stream = futures::stream::unfold(iter, |mut it| async move {
                match it.next_entry() {
                    Ok(Some(entry)) => Some((Ok(entry.to_object_meta()), it)),
                    Ok(None) => None,
                    Err(e) => Some((Err(e), it)),
                }
            });
            return Ok(stream.boxed());
        }
        Ok(self.store.list(prefix))
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
