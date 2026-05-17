//! `object_store` crate-backed implementation of the `Store` trait.
//!
//! Supports `s3://`, `gs://`, `file://` URIs out of the box via the upstream
//! `object_store` 0.13 builders. Range reads use the top-level `get_range`
//! method on the `ObjectStore` trait.

use crate::{ObjectMeta as AsObjectMeta, Store};
use as_core::{Error, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::{BoxStream, StreamExt};
use object_store::{path::Path, ObjectStore, ObjectStoreExt, PutPayload};
use std::ops::Range;
use std::sync::Arc;

pub struct ObjectStoreImpl {
    inner: Arc<dyn ObjectStore>,
    bucket_or_root: String,
}

impl ObjectStoreImpl {
    pub fn new(inner: Arc<dyn ObjectStore>, bucket_or_root: impl Into<String>) -> Self {
        Self {
            inner,
            bucket_or_root: bucket_or_root.into(),
        }
    }

    fn path(&self, key: &str) -> Path {
        Path::from(key.trim_start_matches('/'))
    }

    fn inner(&self) -> &dyn ObjectStore {
        self.inner.as_ref()
    }
}

#[async_trait]
impl Store for ObjectStoreImpl {
    async fn get(&self, key: &str) -> Result<Bytes> {
        let r = self
            .inner()
            .get(&self.path(key))
            .await
            .map_err(|e| Error::Store(e.to_string()))?;
        r.bytes().await.map_err(|e| Error::Store(e.to_string()))
    }

    async fn get_range(&self, key: &str, range: Range<u64>) -> Result<Bytes> {
        self.inner()
            .get_range(&self.path(key), range)
            .await
            .map_err(|e| Error::Store(e.to_string()))
    }

    async fn put(&self, key: &str, data: Bytes) -> Result<()> {
        self.inner()
            .put(&self.path(key), PutPayload::from_bytes(data))
            .await
            .map_err(|e| Error::Store(e.to_string()))?;
        Ok(())
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.inner()
            .delete(&self.path(key))
            .await
            .map_err(|e| Error::Store(e.to_string()))?;
        Ok(())
    }

    async fn head(&self, key: &str) -> Result<AsObjectMeta> {
        let m = self
            .inner()
            .head(&self.path(key))
            .await
            .map_err(|e| Error::Store(e.to_string()))?;
        Ok(AsObjectMeta {
            key: m.location.to_string(),
            size: m.size,
            etag: m.e_tag,
            last_modified: Some(m.last_modified.timestamp()),
        })
    }

    fn list<'a>(&'a self, prefix: &'a str) -> BoxStream<'a, Result<AsObjectMeta>> {
        let p = Path::from(prefix.trim_start_matches('/'));
        self.inner()
            .list(Some(&p))
            .map(|r| {
                r.map(|m| AsObjectMeta {
                    key: m.location.to_string(),
                    size: m.size,
                    etag: m.e_tag,
                    last_modified: Some(m.last_modified.timestamp()),
                })
                .map_err(|e| Error::Store(e.to_string()))
            })
            .boxed()
    }

    fn describe(&self) -> String {
        self.bucket_or_root.clone()
    }
}

impl ObjectStoreImpl {
    pub fn root(&self) -> &str {
        &self.bucket_or_root
    }
}
