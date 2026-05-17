//! Object-store abstraction. Uniform API over S3, GCS, R2, local disk.
//!
//! Hot reads will eventually coalesce range requests and cache to local NVMe
//! (LRU) — see `as-cache`. This crate is the substrate beneath that cache.

use as_core::{Error, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use object_store::{
    aws::AmazonS3Builder, gcp::GoogleCloudStorageBuilder, local::LocalFileSystem, ObjectStore,
};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectMeta {
    pub key: String,
    pub size: u64,
    pub etag: Option<String>,
    pub last_modified: Option<i64>,
}

#[async_trait]
pub trait Store: Send + Sync {
    async fn get(&self, key: &str) -> Result<Bytes>;
    /// Read an object when the caller already has fresh listing/head
    /// metadata for it. Caches can use this to validate without an
    /// additional HEAD request on the hot search path.
    async fn get_fresh(&self, meta: &ObjectMeta) -> Result<Bytes> {
        self.get(&meta.key).await
    }
    async fn get_range(&self, key: &str, range: Range<u64>) -> Result<Bytes>;
    async fn put(&self, key: &str, data: Bytes) -> Result<()>;
    async fn delete(&self, key: &str) -> Result<()>;
    async fn head(&self, key: &str) -> Result<ObjectMeta>;
    fn list<'a>(&'a self, prefix: &'a str) -> BoxStream<'a, Result<ObjectMeta>>;
    /// Free-form description for tracing ("s3://bucket", "file:/path", …).
    fn describe(&self) -> String;
}

pub type ArcStore = Arc<dyn Store>;

pub mod object_store_impl;
pub use object_store_impl::ObjectStoreImpl;

/// Parsed URI: `(scheme, authority, key_prefix)`.
///
/// - `s3://bucket/key`     -> ("s3",  Some("bucket"),    "key")
/// - `gs://bucket/key`     -> ("gs",  Some("bucket"),    "key")
/// - `r2://bucket/key`     -> ("r2",  Some("bucket"),    "key")
/// - `file:///path/x`      -> ("file", None,             "/path/x")
/// - `file://relative`     -> ("file", None,             "relative")
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedUri {
    pub scheme: String,
    pub authority: Option<String>,
    pub key: String,
}

pub fn parse_uri(uri: &str) -> Result<ParsedUri> {
    let (scheme, rest) = uri
        .split_once("://")
        .ok_or_else(|| Error::Config(format!("bad uri: {uri}")))?;
    let scheme = scheme.to_string();

    // `file://` has no authority; everything after the `://` is the path.
    if scheme == "file" {
        return Ok(ParsedUri {
            scheme,
            authority: None,
            key: rest.to_string(),
        });
    }

    // s3/gs/r2: first segment is the bucket, the rest is the key.
    let (authority, key) = match rest.split_once('/') {
        Some((a, k)) => (a.to_string(), k.to_string()),
        None => (rest.to_string(), String::new()),
    };
    Ok(ParsedUri {
        scheme,
        authority: Some(authority),
        key,
    })
}

/// Construct a `Store` from a URI. Reads creds from environment for
/// remote schemes; `file://` uses the local filesystem.
///
/// Returns `(store, key_prefix)` where `key_prefix` is the portion of the
/// URI inside the bucket / root. The caller passes that prefix back to
/// `Store::list`, `Store::get`, etc.
pub fn open(uri: &str) -> Result<(ArcStore, String)> {
    let parsed = parse_uri(uri)?;
    match parsed.scheme.as_str() {
        "file" => {
            let path = PathBuf::from(&parsed.key);
            let root = if path.is_absolute() {
                path
            } else {
                std::env::current_dir()
                    .map_err(Error::Io)?
                    .join(&parsed.key)
            };
            std::fs::create_dir_all(&root).map_err(Error::Io)?;
            let inner =
                LocalFileSystem::new_with_prefix(&root).map_err(|e| Error::Store(e.to_string()))?;
            let desc = format!("file://{}", root.display());
            Ok((
                Arc::new(ObjectStoreImpl::new(
                    Arc::new(inner) as Arc<dyn ObjectStore>,
                    desc,
                )),
                String::new(),
            ))
        }
        "s3" | "r2" => {
            let bucket = parsed
                .authority
                .clone()
                .ok_or_else(|| Error::Config("s3/r2 uri missing bucket".into()))?;
            let mut b = AmazonS3Builder::from_env().with_bucket_name(&bucket);
            if parsed.scheme == "r2" {
                if let Ok(endpoint) = std::env::var("R2_ENDPOINT") {
                    b = b
                        .with_endpoint(endpoint)
                        .with_virtual_hosted_style_request(false);
                }
                if let Ok(account) = std::env::var("R2_ACCOUNT_ID") {
                    b = b.with_endpoint(format!("https://{account}.r2.cloudflarestorage.com"));
                }
            }
            let inner = b.build().map_err(|e| Error::Store(e.to_string()))?;
            let desc = format!("{}://{}", parsed.scheme, bucket);
            Ok((
                Arc::new(ObjectStoreImpl::new(
                    Arc::new(inner) as Arc<dyn ObjectStore>,
                    desc,
                )),
                parsed.key,
            ))
        }
        "gs" => {
            let bucket = parsed
                .authority
                .clone()
                .ok_or_else(|| Error::Config("gs uri missing bucket".into()))?;
            let inner = GoogleCloudStorageBuilder::from_env()
                .with_bucket_name(&bucket)
                .build()
                .map_err(|e| Error::Store(e.to_string()))?;
            let desc = format!("gs://{bucket}");
            Ok((
                Arc::new(ObjectStoreImpl::new(
                    Arc::new(inner) as Arc<dyn ObjectStore>,
                    desc,
                )),
                parsed.key,
            ))
        }
        other => Err(Error::Config(format!("unsupported uri scheme: {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_s3() {
        let p = parse_uri("s3://my-bucket/path/to/file").unwrap();
        assert_eq!(p.scheme, "s3");
        assert_eq!(p.authority.as_deref(), Some("my-bucket"));
        assert_eq!(p.key, "path/to/file");
    }

    #[test]
    fn parse_s3_bucket_only() {
        let p = parse_uri("s3://my-bucket").unwrap();
        assert_eq!(p.scheme, "s3");
        assert_eq!(p.authority.as_deref(), Some("my-bucket"));
        assert_eq!(p.key, "");
    }

    #[test]
    fn parse_file() {
        let p = parse_uri("file:///abs/path").unwrap();
        assert_eq!(p.scheme, "file");
        assert_eq!(p.authority, None);
        assert_eq!(p.key, "/abs/path");
    }

    #[test]
    fn parse_bad() {
        assert!(parse_uri("nope").is_err());
    }
}
