//! Shared server state.
//!
//! Caches one `Tiered`-wrapped store per `(scheme, authority)` pair so the
//! in-process memory LRU survives across REST / MCP calls. Without this
//! caching, every request opens a fresh store and a fresh memory LRU; the
//! cache then only ever helps on the NVMe layer and the "warm" tier is
//! effectively a lie.

use as_ast::SpanCache;
use as_cache::TierConfig;
use as_core::Result;
use as_fs::Fs;
use as_store::ArcStore;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

pub struct AppState {
    tier: TierConfig,
    stores: Mutex<HashMap<String, ArcStore>>,
    /// Shared AST parse cache so repeated `grep --ast` requests against
    /// the same prefix don't reparse unchanged files.
    pub ast: Arc<SpanCache>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            tier: TierConfig::default(),
            stores: Mutex::new(HashMap::new()),
            ast: Arc::new(SpanCache::default()),
        }
    }
}

impl AppState {
    pub fn new(tier: TierConfig) -> Self {
        Self {
            tier,
            stores: Mutex::new(HashMap::new()),
            ast: Arc::new(SpanCache::default()),
        }
    }

    /// Open a URI. The returned `Fs` shares a single tier-wrapped store
    /// with every other call that targets the same `(scheme, authority)`
    /// for object stores; `file://` always opens fresh because
    /// `LocalMmapStore::new` is cheap and the URI may resolve to a
    /// single file (parent + basename) or a directory root.
    pub fn open_fs(&self, uri: &str) -> Result<(Arc<Fs>, String)> {
        let parsed = as_store::parse_uri(uri)?;
        if parsed.scheme == "file" {
            // `as_store::open` already handles file-vs-dir detection
            // and returns the correct (root, key) split. Forwarding it
            // verbatim is the only way `/read file:///tmp/x.txt` and
            // `/grep file:///tmp/` both work.
            let (store, prefix) = as_store::open(uri)?;
            return Ok((Arc::new(Fs::new(store)), prefix));
        }
        // Object stores: cache per (scheme, authority) so the tier
        // cache state survives across requests.
        let cache_key = format!(
            "{}://{}",
            parsed.scheme,
            parsed.authority.as_deref().unwrap_or("")
        );
        let store = {
            let mut map = self.stores.lock();
            if let Some(existing) = map.get(&cache_key) {
                existing.clone()
            } else {
                let (raw, _) = as_store::open(uri)?;
                let wrapped = as_cache::wrap(raw, self.tier.clone());
                map.insert(cache_key, wrapped.clone());
                wrapped
            }
        };
        Ok((Arc::new(Fs::new(store)), parsed.key))
    }
}
