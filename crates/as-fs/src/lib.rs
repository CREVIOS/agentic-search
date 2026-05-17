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
    ///
    /// Robustness: any manifest defect — body truncation, mid-stream
    /// gzip CRC error, short entry count vs. header — causes us to
    /// fall through to live `list` *before* a single manifest entry
    /// is exposed to the caller. Search never silently returns a
    /// partial corpus because the manifest upload was interrupted.
    ///
    /// Cost: we pre-drain the iterator into a `Vec<ManifestEntry>`
    /// (bounded by `header.count`) before streaming, so peak memory
    /// for the manifest is `count * sizeof(ManifestEntry)` rather
    /// than a single entry. For million-entry manifests this is on
    /// the order of 100 MB, well below the bytes we already had to
    /// hold in memory for the gzipped manifest GET itself.
    pub async fn list_with_manifest<'a>(
        &'a self,
        prefix: &'a str,
    ) -> Result<BoxStream<'a, Result<ObjectMeta>>> {
        match as_store::manifest::stream_manifest(&*self.store, prefix).await? {
            Some((header, mut iter)) => {
                let expected = header.count as usize;
                let mut entries: Vec<as_store::manifest::ManifestEntry> =
                    Vec::with_capacity(expected.min(1_000_000));
                loop {
                    match iter.next_entry() {
                        Ok(Some(e)) => entries.push(e),
                        Ok(None) => break,
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "manifest body unreadable mid-stream; falling back to live list",
                            );
                            return Ok(self.store.list(prefix));
                        }
                    }
                }
                if expected > 0 && entries.len() < expected {
                    tracing::warn!(
                        expected,
                        got = entries.len(),
                        "manifest truncated (short count); falling back to live list",
                    );
                    return Ok(self.store.list(prefix));
                }
                // Manifest is whole. Stream entries from the Vec; the
                // unfold owns it so the caller sees a normal listing
                // stream.
                let stream = futures::stream::iter(
                    entries
                        .into_iter()
                        .map(|e| Ok::<_, Error>(e.to_object_meta())),
                );
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    /// A short-count manifest must trigger fallback to live `list`,
    /// not a partial corpus. The previous fix only terminated the
    /// stream after one error — `as-grep`'s "log and continue"
    /// consumer would then return whatever entries did make it
    /// through. Now we drain the manifest fully, detect the short
    /// count, and live-list the real keys.
    #[tokio::test]
    async fn truncated_manifest_falls_back_to_live_list() {
        use bytes::Bytes;
        use std::io::Write;
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let uri = format!("file://{}", dir.path().display());
        let (store, _) = as_store::open(&uri).unwrap();

        // Real files in the prefix: a, b, c.
        for k in ["docs/a.md", "docs/b.md", "docs/c.md"] {
            store.put(k, Bytes::from_static(b"x")).await.unwrap();
        }

        // Fake manifest claims count=10 but only carries 1 body entry.
        let header = serde_json::json!({
            "v": 1, "prefix": "docs", "generated_at": 0, "count": 10
        });
        let entry = serde_json::json!({"key": "docs/a.md", "size": 1});
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        writeln!(gz, "{header}").unwrap();
        writeln!(gz, "{entry}").unwrap();
        let bytes = gz.finish().unwrap();
        store
            .put("docs/.agentic-search/manifest.jsonl.gz", Bytes::from(bytes))
            .await
            .unwrap();

        let fs = Fs::new(store);
        let mut stream = fs.list_with_manifest("docs").await.unwrap();
        let mut keys: Vec<String> = Vec::new();
        while let Some(item) = stream.next().await {
            let meta = item.expect("fallback list should not surface manifest errors");
            keys.push(meta.key);
        }
        keys.sort();
        // After fallback, we get the real corpus (a/b/c plus the
        // manifest file itself; `read_manifest` skips manifest, but
        // live list does include it). Just assert the three real
        // docs are present — fallback worked.
        assert!(keys.contains(&"docs/a.md".to_string()), "{keys:?}");
        assert!(keys.contains(&"docs/b.md".to_string()), "{keys:?}");
        assert!(keys.contains(&"docs/c.md".to_string()), "{keys:?}");
    }

    /// Mid-stream JSON parse failure (corrupt body) is also a
    /// fallback trigger, not a one-error stream.
    #[tokio::test]
    async fn corrupt_manifest_body_falls_back() {
        use bytes::Bytes;
        use std::io::Write;
        use tempfile::tempdir;
        let dir = tempdir().unwrap();
        let uri = format!("file://{}", dir.path().display());
        let (store, _) = as_store::open(&uri).unwrap();
        store
            .put("docs/real.md", Bytes::from_static(b"x"))
            .await
            .unwrap();

        // Header is valid but the body contains a bad line. The
        // streaming reader tolerates bad lines (logs and skips), so
        // this case actually produces fewer entries than `count`
        // and is caught by the short-count check.
        let header = serde_json::json!({
            "v": 1, "prefix": "docs", "generated_at": 0, "count": 3
        });
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        writeln!(gz, "{header}").unwrap();
        writeln!(gz, "this is not json").unwrap();
        writeln!(gz, "nor is this").unwrap();
        let bytes = gz.finish().unwrap();
        store
            .put("docs/.agentic-search/manifest.jsonl.gz", Bytes::from(bytes))
            .await
            .unwrap();

        let fs = Fs::new(store);
        let mut stream = fs.list_with_manifest("docs").await.unwrap();
        let mut saw_real = false;
        while let Some(item) = stream.next().await {
            if let Ok(m) = item {
                if m.key == "docs/real.md" {
                    saw_real = true;
                }
            }
        }
        assert!(saw_real, "fallback live list must include the real file");
    }
}
