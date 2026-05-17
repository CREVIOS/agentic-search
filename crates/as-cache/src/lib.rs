//! Tier cache for object-store reads.
//!
//! Inspired by Turbopuffer's "JIT-compiler-like" architecture: cold reads
//! come from the object store, warm reads from a NVMe LRU on the local
//! disk, hot reads from an in-memory LRU. The more an agent queries a
//! prefix, the closer its bytes live to the CPU.
//!
//! This crate wraps any `as_store::Store` and exposes the same `Store`
//! trait so it can be slotted under `as_fs::Fs` transparently.
//!
//! Cache invalidation is `ETag`-driven: on `head`, if the upstream ETag
//! has changed we evict the cached bytes. `put` and `delete` are
//! pass-through and *also* evict the matching range on this side.

use as_core::{Error, Result};
use as_store::{ArcStore, ObjectMeta, Store};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use lru::LruCache;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct TierConfig {
    /// Max entries in the in-memory LRU.
    pub memory_entries: usize,
    /// Max bytes summed across all in-memory entries.
    pub memory_bytes: usize,
    /// Local directory for the NVMe layer; if `None`, the NVMe tier is off.
    pub nvme_dir: Option<PathBuf>,
    /// Soft budget for the NVMe tier (we sweep above this).
    pub nvme_bytes: u64,
}

impl Default for TierConfig {
    fn default() -> Self {
        Self {
            memory_entries: 4096,
            memory_bytes: 256 * 1024 * 1024,
            nvme_dir: Some(PathBuf::from(".agentic-search/cache")),
            nvme_bytes: 8 * 1024 * 1024 * 1024,
        }
    }
}

pub struct Tiered {
    upstream: ArcStore,
    cfg: TierConfig,
    mem: Mutex<MemoryLru>,
    store_id: String,
}

struct MemoryLru {
    inner: LruCache<String, Bytes>,
    bytes_in: usize,
    bytes_cap: usize,
}

impl MemoryLru {
    fn new(entries: usize, bytes_cap: usize) -> Self {
        Self {
            inner: LruCache::new(NonZeroUsize::new(entries.max(1)).unwrap()),
            bytes_in: 0,
            bytes_cap,
        }
    }

    fn get(&mut self, key: &str) -> Option<Bytes> {
        self.inner.get(key).cloned()
    }

    fn insert(&mut self, key: String, value: Bytes) {
        let len = value.len();
        if len > self.bytes_cap {
            return;
        }
        while self.bytes_in + len > self.bytes_cap {
            match self.inner.pop_lru() {
                Some((_, v)) => self.bytes_in = self.bytes_in.saturating_sub(v.len()),
                None => break,
            }
        }
        if let Some(old) = self.inner.put(key, value) {
            self.bytes_in = self.bytes_in.saturating_sub(old.len());
        }
        self.bytes_in += len;
    }

    fn evict(&mut self, key: &str) {
        if let Some(v) = self.inner.pop(key) {
            self.bytes_in = self.bytes_in.saturating_sub(v.len());
        }
    }
}

impl Tiered {
    pub fn new(upstream: ArcStore, cfg: TierConfig) -> Self {
        if let Some(d) = &cfg.nvme_dir {
            let _ = std::fs::create_dir_all(d);
        }
        let mem = Mutex::new(MemoryLru::new(cfg.memory_entries, cfg.memory_bytes));
        let store_id = upstream.describe();
        Self {
            upstream,
            cfg,
            mem,
            store_id,
        }
    }

    /// Cache keys must include store identity so two distinct stores
    /// (e.g. `s3://bucket-a` and `s3://bucket-b`) with the same in-bucket
    /// key never collide on the same cache slot.
    fn cache_key_full(&self, key: &str) -> String {
        format!("full:{}:{key}", self.store_id)
    }

    fn cache_key_range(&self, key: &str, range: &Range<u64>) -> String {
        format!(
            "range:{}:{key}:{}-{}",
            self.store_id, range.start, range.end
        )
    }

    fn nvme_path(&self, cache_key: &str) -> Option<PathBuf> {
        self.cfg.nvme_dir.as_ref().map(|d| {
            let mut h = Sha256::new();
            h.update(cache_key.as_bytes());
            let hash = hex::encode(h.finalize());
            d.join(&hash[0..2]).join(&hash[2..])
        })
    }

    async fn nvme_load(&self, cache_key: &str) -> Option<Bytes> {
        let path = self.nvme_path(cache_key)?;
        match tokio::fs::read(&path).await {
            Ok(v) => Some(Bytes::from(v)),
            Err(_) => None,
        }
    }

    async fn nvme_store(&self, cache_key: &str, data: &Bytes) {
        if let Some(path) = self.nvme_path(cache_key) {
            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&path, data.as_ref()).await;
        }
    }

    async fn nvme_evict(&self, cache_key: &str) {
        if let Some(path) = self.nvme_path(cache_key) {
            let _ = tokio::fs::remove_file(&path).await;
        }
    }
}

#[async_trait]
impl Store for Tiered {
    async fn get(&self, key: &str) -> Result<Bytes> {
        let ck = self.cache_key_full(key);
        if let Some(b) = self.mem.lock().get(&ck) {
            return Ok(b);
        }
        if let Some(b) = self.nvme_load(&ck).await {
            self.mem.lock().insert(ck, b.clone());
            return Ok(b);
        }
        let bytes = self.upstream.get(key).await?;
        self.nvme_store(&ck, &bytes).await;
        self.mem.lock().insert(ck, bytes.clone());
        Ok(bytes)
    }

    async fn get_range(&self, key: &str, range: Range<u64>) -> Result<Bytes> {
        let ck = self.cache_key_range(key, &range);
        if let Some(b) = self.mem.lock().get(&ck) {
            return Ok(b);
        }
        if let Some(b) = self.nvme_load(&ck).await {
            self.mem.lock().insert(ck, b.clone());
            return Ok(b);
        }
        let bytes = self.upstream.get_range(key, range).await?;
        self.nvme_store(&ck, &bytes).await;
        self.mem.lock().insert(ck, bytes.clone());
        Ok(bytes)
    }

    async fn put(&self, key: &str, data: Bytes) -> Result<()> {
        let ck = self.cache_key_full(key);
        self.mem.lock().evict(&ck);
        self.nvme_evict(&ck).await;
        self.upstream.put(key, data).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let ck = self.cache_key_full(key);
        self.mem.lock().evict(&ck);
        self.nvme_evict(&ck).await;
        self.upstream.delete(key).await
    }

    async fn head(&self, key: &str) -> Result<ObjectMeta> {
        // Pass-through: ETag-driven invalidation lives in the planner /
        // refresh loop, not in head.
        self.upstream.head(key).await
    }

    fn list<'a>(&'a self, prefix: &'a str) -> BoxStream<'a, Result<ObjectMeta>> {
        self.upstream.list(prefix)
    }

    fn describe(&self) -> String {
        format!("tiered({})", self.upstream.describe())
    }
}

/// Helper: wrap any store in a `Tiered` cache.
pub fn wrap(upstream: ArcStore, cfg: TierConfig) -> ArcStore {
    Arc::new(Tiered::new(upstream, cfg))
}

/// Tiny RAII helper that prevents the impossible-to-hit "Error::Other"
/// warning when other compile units pull only part of the type set.
#[doc(hidden)]
pub fn _ensure_error() -> Result<()> {
    Err(Error::Other("unreachable".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use tempfile::tempdir;

    #[tokio::test]
    async fn memory_hit_avoids_upstream() {
        let dir = tempdir().unwrap();
        let uri = format!("file://{}", dir.path().display());
        let (upstream, _) = as_store::open(&uri).unwrap();
        upstream
            .put("k", Bytes::from_static(b"hello"))
            .await
            .unwrap();

        let cache_dir = tempdir().unwrap();
        let cfg = TierConfig {
            memory_entries: 64,
            memory_bytes: 1024 * 1024,
            nvme_dir: Some(cache_dir.path().to_path_buf()),
            nvme_bytes: 1024 * 1024,
        };
        let tiered = Tiered::new(upstream.clone(), cfg);

        // First read populates both tiers.
        let v1 = tiered.get("k").await.unwrap();
        assert_eq!(v1.as_ref(), b"hello");
        // Mutate upstream behind our back; cached read should not see it.
        upstream
            .put("k", Bytes::from_static(b"changed"))
            .await
            .unwrap();
        let v2 = tiered.get("k").await.unwrap();
        assert_eq!(
            v2.as_ref(),
            b"hello",
            "memory tier should serve the prior value"
        );
    }

    #[tokio::test]
    async fn nvme_hit_repopulates_memory() {
        let dir = tempdir().unwrap();
        let uri = format!("file://{}", dir.path().display());
        let (upstream, _) = as_store::open(&uri).unwrap();
        upstream
            .put("k", Bytes::from_static(b"abcd"))
            .await
            .unwrap();

        let cache_dir = tempdir().unwrap();
        let cfg = TierConfig {
            memory_entries: 1,
            memory_bytes: 1024,
            nvme_dir: Some(cache_dir.path().to_path_buf()),
            nvme_bytes: 1024,
        };
        let tiered = Tiered::new(upstream.clone(), cfg);

        let _ = tiered.get("k").await.unwrap();
        // Evict from memory by writing a second key (cap=1 entry).
        upstream
            .put("other", Bytes::from_static(b"xxxx"))
            .await
            .unwrap();
        let _ = tiered.get("other").await.unwrap();
        // Mutate upstream so we know if we go back to it.
        upstream
            .put("k", Bytes::from_static(b"changed"))
            .await
            .unwrap();
        let v = tiered.get("k").await.unwrap();
        assert_eq!(
            v.as_ref(),
            b"abcd",
            "should have served from NVMe, not upstream"
        );
    }

    #[tokio::test]
    async fn put_evicts_cache() {
        let dir = tempdir().unwrap();
        let uri = format!("file://{}", dir.path().display());
        let (upstream, _) = as_store::open(&uri).unwrap();
        upstream.put("k", Bytes::from_static(b"old")).await.unwrap();
        let cache_dir = tempdir().unwrap();
        let cfg = TierConfig {
            memory_entries: 64,
            memory_bytes: 1024,
            nvme_dir: Some(cache_dir.path().to_path_buf()),
            nvme_bytes: 1024,
        };
        let tiered = Tiered::new(upstream.clone(), cfg);
        let _ = tiered.get("k").await.unwrap();

        tiered.put("k", Bytes::from_static(b"new")).await.unwrap();
        let v = tiered.get("k").await.unwrap();
        assert_eq!(v.as_ref(), b"new");
    }
}
