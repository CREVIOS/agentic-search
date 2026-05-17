//! Object-store abstraction. Uniform API over S3, GCS, R2, local disk.
//!
//! Hot reads coalesce range requests and cache to local NVMe (LRU).

use as_core::{Error, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use std::ops::Range;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct ObjectMeta {
    pub key: String,
    pub size: u64,
    pub etag: Option<String>,
    pub last_modified: Option<i64>,
}

#[async_trait]
pub trait Store: Send + Sync {
    async fn get(&self, key: &str) -> Result<Bytes>;
    async fn get_range(&self, key: &str, range: Range<u64>) -> Result<Bytes>;
    async fn put(&self, key: &str, data: Bytes) -> Result<()>;
    async fn delete(&self, key: &str) -> Result<()>;
    async fn head(&self, key: &str) -> Result<ObjectMeta>;
    fn list<'a>(&'a self, prefix: &'a str) -> BoxStream<'a, Result<ObjectMeta>>;
}

pub type ArcStore = Arc<dyn Store>;

pub mod object_store_impl;

/// Parse a URI like `s3://bucket/prefix`, `gs://bucket/prefix`, `file:///path`.
pub fn parse_uri(uri: &str) -> Result<(String, String)> {
    let (scheme, rest) = uri
        .split_once("://")
        .ok_or_else(|| Error::Config(format!("bad uri: {uri}")))?;
    Ok((scheme.to_string(), rest.to_string()))
}
