//! Shared server state.
//!
//! Caches one `Tiered`-wrapped store per `(scheme, authority)` pair so the
//! in-process memory LRU survives across REST / MCP calls. Without this
//! caching, every request opens a fresh store and a fresh memory LRU; the
//! cache then only ever helps on the NVMe layer and the "warm" tier is
//! effectively a lie.

use as_cache::TierConfig;
use as_core::Result;
use as_fs::Fs;
use as_store::ArcStore;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Default)]
pub struct AppState {
    tier: TierConfig,
    stores: Mutex<HashMap<String, ArcStore>>,
}

impl AppState {
    pub fn new(tier: TierConfig) -> Self {
        Self {
            tier,
            stores: Mutex::new(HashMap::new()),
        }
    }

    /// Open a URI. The returned `Fs` shares a single tier-wrapped store
    /// with every other call that targets the same `(scheme, authority)`.
    /// For `file://` we key on the full root path so two different local
    /// roots stay independent.
    pub fn open_fs(&self, uri: &str) -> Result<(Arc<Fs>, String)> {
        let parsed = as_store::parse_uri(uri)?;
        let cache_key = match parsed.scheme.as_str() {
            "file" => format!("file://{}", parsed.key),
            other => format!("{other}://{}", parsed.authority.as_deref().unwrap_or("")),
        };
        // Open the upstream store (or reuse a cached one).
        let store = {
            let mut map = self.stores.lock();
            if let Some(existing) = map.get(&cache_key) {
                existing.clone()
            } else {
                let (raw, _root_prefix) = as_store::open(uri)?;
                let wrapped = if parsed.scheme == "file" {
                    raw
                } else {
                    as_cache::wrap(raw, self.tier.clone())
                };
                map.insert(cache_key, wrapped.clone());
                wrapped
            }
        };
        // The in-store key prefix for *this* call comes from the parsed
        // URI (S3/R2/GCS: object key under the bucket; file: empty
        // because the LocalFileSystem store is already rooted).
        let prefix = match parsed.scheme.as_str() {
            "file" => String::new(),
            _ => parsed.key,
        };
        Ok((Arc::new(Fs::new(store)), prefix))
    }
}
