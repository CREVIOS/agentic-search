//! Shared server state: tier-cache configuration and a store opener.
//!
//! For now stores are opened per-request. The tier cache is what
//! actually keeps cold reads from S3 hot across calls; once we have
//! cross-request sharing requirements we can re-introduce a store cache
//! keyed by (scheme, authority, root) — the previous attempt collided
//! `file://` per-path roots so it has been backed out.

use as_cache::TierConfig;
use as_core::Result;
use as_fs::Fs;
use std::sync::Arc;

#[derive(Default)]
pub struct AppState {
    tier: TierConfig,
}

impl AppState {
    pub fn new(tier: TierConfig) -> Self {
        Self { tier }
    }

    /// Open a URI, return a tiered FS plus the in-store key prefix.
    pub fn open_fs(&self, uri: &str) -> Result<(Arc<Fs>, String)> {
        let (store, prefix) = as_store::open(uri)?;
        let store = as_cache::wrap(store, self.tier.clone());
        Ok((Arc::new(Fs::new(store)), prefix))
    }
}
