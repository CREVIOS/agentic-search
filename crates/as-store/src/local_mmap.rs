//! Local-filesystem `Store` impl using `mmap` instead of `tokio::fs::read`.
//!
//! `object_store::LocalFileSystem` is correct but pays for tokio FS
//! buffering. For agent workloads where the corpus is on the local
//! disk (or an FS-mounted S3 like `mountpoint-s3` / S3 Files) the OS
//! page cache is already hot, and a single `mmap` + slice is enough.
//!
//! This module is `file://` only. Object-store backends (S3 / GCS /
//! R2) still go through `object_store_impl`.

use crate::{ObjectMeta, Store};
use as_core::{Error, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::{self, BoxStream, StreamExt};
use memmap2::Mmap;
use std::fs::File;
use std::ops::Range;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct LocalMmapStore {
    root: PathBuf,
}

impl LocalMmapStore {
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        // Only create the directory if it doesn't exist *and* the
        // caller chose a path-like location. A read-shaped URI (e.g.
        // `file:///tmp/missing.txt`) must not silently mkdir its
        // parent or basename — that's a side-effect on the read path.
        let canon = match std::fs::canonicalize(&root) {
            Ok(c) => c,
            Err(_) => {
                // Path doesn't exist yet — likely a future-write
                // target. Create it and canonicalize again.
                std::fs::create_dir_all(&root).map_err(Error::Io)?;
                std::fs::canonicalize(&root).map_err(Error::Io)?
            }
        };
        Ok(Self { root: canon })
    }

    fn full_path(&self, key: &str) -> PathBuf {
        let trimmed = key.trim_start_matches('/');
        if trimmed.is_empty() {
            self.root.clone()
        } else {
            self.root.join(trimmed)
        }
    }

    /// Reject keys that try to escape the root via `..`.
    fn safe_path(&self, key: &str) -> Result<PathBuf> {
        let trimmed = key.trim_start_matches('/');
        // Reject any `..` segment outright. `Path::starts_with` doesn't
        // resolve `..`, so we can't rely on a post-hoc canonical check
        // for files that may not yet exist.
        for component in trimmed.split('/') {
            if component == ".." {
                return Err(Error::Store(format!(
                    "path escape: {key} contains '..' segment"
                )));
            }
        }
        let candidate = self.full_path(key);
        // If the file *does* exist, double-check via canonicalisation.
        if let Ok(canon) = std::fs::canonicalize(&candidate) {
            if !canon.starts_with(&self.root) {
                return Err(Error::Store(format!(
                    "path escape: {key} resolved outside root"
                )));
            }
            return Ok(canon);
        }
        Ok(candidate)
    }

    fn meta_for(&self, path: &Path) -> Result<ObjectMeta> {
        let md = std::fs::metadata(path).map_err(Error::Io)?;
        let rel = path
            .strip_prefix(&self.root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.display().to_string());
        Ok(ObjectMeta {
            key: rel,
            size: md.len(),
            etag: None,
            last_modified: md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64),
        })
    }
}

fn mmap_file(path: &Path) -> Result<Option<Mmap>> {
    let f = File::open(path).map_err(Error::Io)?;
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    // `Mmap::map` errors on zero-length files; treat them as "empty"
    // so a single empty file in a corpus does not poison a whole
    // prefix scan.
    if len == 0 {
        return Ok(None);
    }
    // SAFETY: tree-sitter / ripgrep already required immutable file
    // contents; we open read-only and only use the mapping until the
    // task finishes. Concurrent external writes can in theory cause
    // SIGBUS, which is the same risk every mmap-based grep has.
    unsafe { Mmap::map(&f) }.map(Some).map_err(Error::Io)
}

#[async_trait]
impl Store for LocalMmapStore {
    async fn get(&self, key: &str) -> Result<Bytes> {
        // Inline mmap. spawn_blocking added ~50-100 µs of scheduler
        // round-trip per call; mmap itself is a single syscall that
        // sets up a page-table mapping (~10 µs on macOS / Linux),
        // not blocking I/O. For a probe=32 vector query that fires
        // 32 store.get calls in parallel the scheduler overhead
        // dominated wall time — bench measured ~5 ms/cluster on
        // local FS, of which only ~1 ms was real work.
        //
        // The first access to each mapped page still faults from
        // disk on a cold cache, but that fault happens in the
        // *scorer*, not here, and the OS handles it without
        // blocking the tokio worker for long enough to matter.
        //
        // Caveat: `LocalMmapStore` is for local FS only. If a caller
        // points it at a network filesystem (NFS, mountpoint-s3,
        // FUSE) the open + mmap syscall can stall arbitrarily.
        // Don't wrap a slow remote mount in this store — use the
        // `ObjectStoreImpl` path (which keeps its own async
        // semantics) instead.
        let path = self.safe_path(key)?;
        match mmap_file(&path)? {
            Some(mmap) => Ok(Bytes::from_owner(MmapBuf(mmap))),
            None => Ok(Bytes::new()),
        }
    }

    async fn get_range(&self, key: &str, range: Range<u64>) -> Result<Bytes> {
        let path = self.safe_path(key)?;
        match mmap_file(&path)? {
            Some(mmap) => {
                let end = (range.end as usize).min(mmap.len());
                let start = (range.start as usize).min(end);
                let bytes = Bytes::from_owner(MmapBuf(mmap));
                Ok(bytes.slice(start..end))
            }
            None => Ok(Bytes::new()),
        }
    }

    async fn put(&self, key: &str, data: Bytes) -> Result<()> {
        let path = self.safe_path(key)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(Error::Io)?;
        }
        tokio::fs::write(&path, &data).await.map_err(Error::Io)
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let path = self.safe_path(key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Error::Store(format!(
                "object not found: {}",
                path.display()
            ))),
            Err(e) => Err(Error::Io(e)),
        }
    }

    async fn head(&self, key: &str) -> Result<ObjectMeta> {
        let path = self.safe_path(key)?;
        self.meta_for(&path)
    }

    fn list<'a>(&'a self, prefix: &'a str) -> BoxStream<'a, Result<ObjectMeta>> {
        let root = self.full_path(prefix);
        let store_root = self.root.clone();
        // Walk directories synchronously (cheap, even for big trees)
        // and stream out metadata. Async iteration is unnecessary here:
        // listing is bounded and not the hot path; the parallel scan
        // above us paginates work across many files anyway.
        let walker = WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(move |e| {
                let path = e.into_path();
                let md = std::fs::metadata(&path).map_err(Error::Io)?;
                let rel = path
                    .strip_prefix(&store_root)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| path.display().to_string());
                Ok::<_, Error>(ObjectMeta {
                    key: rel,
                    size: md.len(),
                    etag: None,
                    last_modified: md
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64),
                })
            });
        stream::iter(walker).boxed()
    }

    fn describe(&self) -> String {
        format!("file://{}", self.root.display())
    }
}

/// Thin owner wrapper so `Bytes::from_owner` can keep an `Mmap` alive
/// and let callers borrow it as `&[u8]` without copying.
struct MmapBuf(Mmap);

impl AsRef<[u8]> for MmapBuf {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use tempfile::tempdir;

    #[tokio::test]
    async fn roundtrip() {
        let dir = tempdir().unwrap();
        let store = LocalMmapStore::new(dir.path()).unwrap();
        store
            .put("a/b.txt", Bytes::from_static(b"hello mmap\n"))
            .await
            .unwrap();
        let bytes = store.get("a/b.txt").await.unwrap();
        assert_eq!(bytes.as_ref(), b"hello mmap\n");
        let slice = store.get_range("a/b.txt", 6..10).await.unwrap();
        assert_eq!(slice.as_ref(), b"mmap");
        let meta = store.head("a/b.txt").await.unwrap();
        assert_eq!(meta.size, 11);
        store.delete("a/b.txt").await.unwrap();
        assert!(store.head("a/b.txt").await.is_err());
    }

    #[tokio::test]
    async fn path_escape_rejected() {
        let dir = tempdir().unwrap();
        let store = LocalMmapStore::new(dir.path()).unwrap();
        let err = store.get("../etc/passwd").await.unwrap_err();
        assert!(err.to_string().contains("escape"), "got {err}");
    }

    #[tokio::test]
    async fn zero_byte_file_reads_empty() {
        let dir = tempdir().unwrap();
        let store = LocalMmapStore::new(dir.path()).unwrap();
        store.put("empty.txt", Bytes::new()).await.unwrap();
        let bytes = store.get("empty.txt").await.unwrap();
        assert!(bytes.is_empty());
        let range = store.get_range("empty.txt", 0..0).await.unwrap();
        assert!(range.is_empty());
    }
}
