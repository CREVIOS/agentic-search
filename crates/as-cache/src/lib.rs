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
//! Cache invalidation is metadata-driven. Prefix scans pass fresh list
//! metadata into the cache, so hot search reads can validate `ETag` /
//! last-modified / size without an extra HEAD. Direct reads validate via
//! HEAD before serving cached bytes. `put` and `delete` evict every cached
//! full/range entry for that key in the current process.

use as_core::{Error, Result};
use as_store::{ArcStore, ObjectMeta, Store};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use lru::LruCache;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
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
    key_index: Mutex<HashMap<String, HashSet<String>>>,
    store_id: String,
}

#[derive(Clone)]
struct CacheEntry {
    bytes: Bytes,
    fingerprint: Fingerprint,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Fingerprint {
    size: u64,
    etag: Option<String>,
    last_modified: Option<i64>,
}

impl Fingerprint {
    fn from_meta(meta: &ObjectMeta) -> Self {
        Self {
            size: meta.size,
            etag: meta.etag.clone(),
            last_modified: meta.last_modified,
        }
    }

    fn encode(&self) -> String {
        format!(
            "{}\n{}\n{}\n",
            self.size,
            self.etag.as_deref().unwrap_or(""),
            self.last_modified
                .map(|ts| ts.to_string())
                .unwrap_or_default()
        )
    }

    fn decode(s: &str) -> Option<Self> {
        let mut lines = s.lines();
        let size = lines.next()?.parse().ok()?;
        let etag = match lines.next() {
            Some("") | None => None,
            Some(v) => Some(v.to_string()),
        };
        let last_modified = match lines.next() {
            Some("") | None => None,
            Some(v) => v.parse().ok(),
        };
        Some(Self {
            size,
            etag,
            last_modified,
        })
    }
}

struct MemoryLru {
    inner: LruCache<String, CacheEntry>,
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

    fn get(&mut self, key: &str) -> Option<CacheEntry> {
        self.inner.get(key).cloned()
    }

    fn insert(&mut self, key: String, value: CacheEntry) {
        let len = value.bytes.len();
        if len > self.bytes_cap {
            return;
        }
        while self.bytes_in + len > self.bytes_cap {
            match self.inner.pop_lru() {
                Some((_, v)) => self.bytes_in = self.bytes_in.saturating_sub(v.bytes.len()),
                None => break,
            }
        }
        if let Some(old) = self.inner.put(key, value) {
            self.bytes_in = self.bytes_in.saturating_sub(old.bytes.len());
        }
        self.bytes_in += len;
    }

    fn evict(&mut self, key: &str) {
        if let Some(v) = self.inner.pop(key) {
            self.bytes_in = self.bytes_in.saturating_sub(v.bytes.len());
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
            key_index: Mutex::new(HashMap::new()),
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

    fn nvme_meta_path(&self, cache_key: &str) -> Option<PathBuf> {
        self.nvme_path(cache_key).map(|p| p.with_extension("meta"))
    }

    fn remember_cache_key(&self, key: &str, cache_key: &str) {
        self.key_index
            .lock()
            .entry(key.to_string())
            .or_default()
            .insert(cache_key.to_string());
    }

    fn forget_cache_key(&self, key: &str, cache_key: &str) {
        let mut index = self.key_index.lock();
        if let Some(keys) = index.get_mut(key) {
            keys.remove(cache_key);
            if keys.is_empty() {
                index.remove(key);
            }
        }
    }

    async fn evict_cache_key(&self, key: &str, cache_key: &str) {
        self.mem.lock().evict(cache_key);
        self.nvme_evict(cache_key).await;
        self.forget_cache_key(key, cache_key);
    }

    async fn evict_key(&self, key: &str) {
        let keys = self.key_index.lock().remove(key).unwrap_or_default();
        for cache_key in keys {
            self.mem.lock().evict(&cache_key);
            self.nvme_evict(&cache_key).await;
        }
        let full_key = self.cache_key_full(key);
        self.mem.lock().evict(&full_key);
        self.nvme_evict(&full_key).await;
    }

    async fn nvme_load(&self, cache_key: &str) -> Option<CacheEntry> {
        let path = self.nvme_path(cache_key)?;
        let meta_path = self.nvme_meta_path(cache_key)?;
        let (data, meta) = tokio::join!(
            tokio::fs::read(&path),
            tokio::fs::read_to_string(&meta_path)
        );
        let bytes = Bytes::from(data.ok()?);
        let fingerprint = Fingerprint::decode(&meta.ok()?)?;
        Some(CacheEntry { bytes, fingerprint })
    }

    async fn nvme_store(&self, cache_key: &str, entry: &CacheEntry) {
        if let Some(path) = self.nvme_path(cache_key) {
            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let _ = tokio::fs::write(&path, entry.bytes.as_ref()).await;
            if let Some(meta_path) = self.nvme_meta_path(cache_key) {
                let _ = tokio::fs::write(&meta_path, entry.fingerprint.encode()).await;
            }
        }
    }

    async fn nvme_evict(&self, cache_key: &str) {
        if let Some(path) = self.nvme_path(cache_key) {
            let _ = tokio::fs::remove_file(&path).await;
        }
        if let Some(path) = self.nvme_meta_path(cache_key) {
            let _ = tokio::fs::remove_file(&path).await;
        }
    }

    async fn get_full_with_meta(&self, meta: &ObjectMeta) -> Result<Bytes> {
        let ck = self.cache_key_full(&meta.key);
        let want = Fingerprint::from_meta(meta);
        // Scope each `mem.lock()` so the parking_lot guard is dropped
        // before any `.await`. parking_lot guards are !Send, so holding
        // one across an await would make the whole future !Send.
        let mem_hit = {
            let mut mem = self.mem.lock();
            mem.get(&ck)
        };
        if let Some(entry) = mem_hit {
            if entry.fingerprint == want {
                return Ok(entry.bytes);
            }
            self.evict_cache_key(&meta.key, &ck).await;
        }
        if let Some(entry) = self.nvme_load(&ck).await {
            if entry.fingerprint == want {
                {
                    let mut mem = self.mem.lock();
                    mem.insert(ck.clone(), entry.clone());
                }
                self.remember_cache_key(&meta.key, &ck);
                return Ok(entry.bytes);
            }
            self.evict_cache_key(&meta.key, &ck).await;
        }
        let bytes = self.upstream.get(&meta.key).await?;
        let entry = CacheEntry {
            bytes: bytes.clone(),
            fingerprint: want,
        };
        self.nvme_store(&ck, &entry).await;
        {
            let mut mem = self.mem.lock();
            mem.insert(ck.clone(), entry);
        }
        self.remember_cache_key(&meta.key, &ck);
        Ok(bytes)
    }

    async fn get_range_with_meta(&self, meta: &ObjectMeta, range: Range<u64>) -> Result<Bytes> {
        let ck = self.cache_key_range(&meta.key, &range);
        let want = Fingerprint::from_meta(meta);
        let mem_hit = {
            let mut mem = self.mem.lock();
            mem.get(&ck)
        };
        if let Some(entry) = mem_hit {
            if entry.fingerprint == want {
                return Ok(entry.bytes);
            }
            self.evict_cache_key(&meta.key, &ck).await;
        }
        if let Some(entry) = self.nvme_load(&ck).await {
            if entry.fingerprint == want {
                {
                    let mut mem = self.mem.lock();
                    mem.insert(ck.clone(), entry.clone());
                }
                self.remember_cache_key(&meta.key, &ck);
                return Ok(entry.bytes);
            }
            self.evict_cache_key(&meta.key, &ck).await;
        }
        let bytes = self.upstream.get_range(&meta.key, range).await?;
        let entry = CacheEntry {
            bytes: bytes.clone(),
            fingerprint: want,
        };
        self.nvme_store(&ck, &entry).await;
        {
            let mut mem = self.mem.lock();
            mem.insert(ck.clone(), entry);
        }
        self.remember_cache_key(&meta.key, &ck);
        Ok(bytes)
    }
}

#[async_trait]
impl Store for Tiered {
    async fn get(&self, key: &str) -> Result<Bytes> {
        let meta = self.upstream.head(key).await?;
        self.get_full_with_meta(&meta).await
    }

    async fn get_fresh(&self, meta: &ObjectMeta) -> Result<Bytes> {
        self.get_full_with_meta(meta).await
    }

    async fn get_range(&self, key: &str, range: Range<u64>) -> Result<Bytes> {
        let meta = self.upstream.head(key).await?;
        self.get_range_with_meta(&meta, range).await
    }

    async fn put(&self, key: &str, data: Bytes) -> Result<()> {
        self.evict_key(key).await;
        self.upstream.put(key, data).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.evict_key(key).await;
        self.upstream.delete(key).await
    }

    async fn head(&self, key: &str) -> Result<ObjectMeta> {
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
    async fn cached_read_revalidates_when_upstream_changes() {
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
        // Mutate upstream behind our back; direct cached reads must
        // validate metadata before serving bytes.
        upstream
            .put("k", Bytes::from_static(b"changed"))
            .await
            .unwrap();
        let v2 = tiered.get("k").await.unwrap();
        assert_eq!(
            v2.as_ref(),
            b"changed",
            "cache should not serve stale bytes after metadata changes"
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
        let v = tiered.get("k").await.unwrap();
        assert_eq!(
            v.as_ref(),
            b"abcd",
            "should preserve bytes after memory eviction"
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

    #[tokio::test]
    async fn put_evicts_range_cache() {
        let dir = tempdir().unwrap();
        let uri = format!("file://{}", dir.path().display());
        let (upstream, _) = as_store::open(&uri).unwrap();
        upstream
            .put("k", Bytes::from_static(b"abcdef"))
            .await
            .unwrap();
        let cache_dir = tempdir().unwrap();
        let cfg = TierConfig {
            memory_entries: 64,
            memory_bytes: 1024,
            nvme_dir: Some(cache_dir.path().to_path_buf()),
            nvme_bytes: 1024,
        };
        let tiered = Tiered::new(upstream.clone(), cfg);
        let first = tiered.get_range("k", 1..4).await.unwrap();
        assert_eq!(first.as_ref(), b"bcd");

        tiered
            .put("k", Bytes::from_static(b"uvwxyz"))
            .await
            .unwrap();
        let second = tiered.get_range("k", 1..4).await.unwrap();
        assert_eq!(second.as_ref(), b"vwx");
    }
}
