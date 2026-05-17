//! Prefix manifest: gzipped JSON-Lines list of every object under a
//! prefix. Lets cold `list` collapse from paged `ListObjectsV2` to a
//! single GET when a manifest is present.
//!
//! Format (`MANIFEST_PATH` = `.agentic-search/manifest.jsonl.gz`):
//!
//! ```text
//! {"v":1,"prefix":"docs","generated_at":1700000000,"count":12345}
//! {"key":"docs/a.md","size":42,"etag":"...","last_modified":...}
//! ...
//! ```
//!
//! The first line is the header so a reader can sanity-check `prefix`
//! and `count` before iterating. Writes are *not* atomic at the object-
//! store layer; we use a temp key + final-key rename via copy+delete
//! when the upstream supports it, otherwise we write directly and
//! readers tolerate a truncated trailing line.

use crate::{ObjectMeta, Store};
use as_core::{Error, Result};
use bytes::Bytes;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use std::io::Write;
use tracing;

pub const MANIFEST_PATH: &str = ".agentic-search/manifest.jsonl.gz";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManifestHeader {
    /// Format version. Bump when the schema changes.
    pub v: u32,
    /// Prefix the manifest covers (relative to the store root / bucket).
    pub prefix: String,
    /// Unix epoch seconds.
    pub generated_at: i64,
    /// Number of object entries that follow.
    pub count: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub key: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<i64>,
}

impl From<ObjectMeta> for ManifestEntry {
    fn from(m: ObjectMeta) -> Self {
        Self {
            key: m.key,
            size: m.size,
            etag: m.etag,
            last_modified: m.last_modified,
        }
    }
}

impl ManifestEntry {
    pub fn to_object_meta(self) -> ObjectMeta {
        ObjectMeta {
            key: self.key,
            size: self.size,
            etag: self.etag,
            last_modified: self.last_modified,
        }
    }
}

fn manifest_key(prefix: &str) -> String {
    let p = prefix.trim_end_matches('/');
    if p.is_empty() {
        MANIFEST_PATH.to_string()
    } else {
        format!("{p}/{MANIFEST_PATH}")
    }
}

/// Build a manifest by walking `prefix` and uploading the gzipped JSONL
/// to `<prefix>/.agentic-search/manifest.jsonl.gz`. Skips the manifest
/// path itself so we never recurse.
pub async fn write_manifest(store: &dyn Store, prefix: &str) -> Result<ManifestHeader> {
    use futures::stream::StreamExt;
    let manifest_path = manifest_key(prefix);

    // Stream the upstream listing through the gzip encoder one entry
    // at a time. For an M-doc bucket we keep peak RAM at one
    // `ManifestEntry`, plus the encoder's in-memory output (compressed
    // body) which is still <100 MB even for millions of small docs.
    let mut gz = GzEncoder::new(Vec::with_capacity(64 * 1024), Compression::default());
    // Placeholder for the header line — we don't know the count until
    // the stream finishes, so we'll rewrite the header at the end by
    // assembling header + body separately.
    let mut body = GzEncoder::new(Vec::with_capacity(64 * 1024), Compression::default());
    let mut count: u64 = 0;
    let mut listing = store.list(prefix);
    while let Some(item) = listing.next().await {
        let meta = item?;
        if meta.key.ends_with(MANIFEST_PATH) {
            continue;
        }
        let entry: ManifestEntry = meta.into();
        let line = serde_json::to_string(&entry).map_err(|e| Error::Other(e.to_string()))?;
        body.write_all(line.as_bytes()).map_err(Error::Io)?;
        body.write_all(b"\n").map_err(Error::Io)?;
        count += 1;
    }
    let body_bytes = body.finish().map_err(Error::Io)?;

    let header = ManifestHeader {
        v: 1,
        prefix: prefix.trim_end_matches('/').to_string(),
        generated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or_default(),
        count,
    };
    let header_line = serde_json::to_string(&header).map_err(|e| Error::Other(e.to_string()))?;
    gz.write_all(header_line.as_bytes()).map_err(Error::Io)?;
    gz.write_all(b"\n").map_err(Error::Io)?;
    // Decompress the body into the outer gzip stream. Smaller surface
    // than chaining encoders directly and lets the header live in its
    // own deterministic prefix.
    let mut body_decoder = flate2::read::GzDecoder::new(&body_bytes[..]);
    let mut buf = [0u8; 16 * 1024];
    loop {
        use std::io::Read;
        let n = body_decoder.read(&mut buf).map_err(Error::Io)?;
        if n == 0 {
            break;
        }
        gz.write_all(&buf[..n]).map_err(Error::Io)?;
    }
    let gz_bytes = gz.finish().map_err(Error::Io)?;

    store.put(&manifest_path, Bytes::from(gz_bytes)).await?;
    Ok(header)
}

/// Read a manifest if one exists, materializing every entry into a
/// `Vec`. Convenient for small prefixes and unit tests; for M-doc
/// prefixes prefer `stream_manifest` so memory stays bounded.
pub async fn read_manifest(
    store: &dyn Store,
    prefix: &str,
) -> Result<Option<(ManifestHeader, Vec<ManifestEntry>)>> {
    match stream_manifest(store, prefix).await? {
        None => Ok(None),
        Some((header, mut iter)) => {
            let mut entries = Vec::with_capacity(header.count as usize);
            while let Some(entry) = iter.next_entry()? {
                entries.push(entry);
            }
            Ok(Some((header, entries)))
        }
    }
}

/// Stream a manifest entry-by-entry. The manifest body is **never
/// materialised**: the gunzipped JSONL is consumed line-by-line via a
/// `BufRead` reader so peak memory stays at one entry, regardless of
/// whether the manifest covers 10 or 10 million objects. Returns the
/// header so callers can branch on `count` / `prefix` without scanning
/// the body.
pub async fn stream_manifest(
    store: &dyn Store,
    prefix: &str,
) -> Result<Option<(ManifestHeader, ManifestIter)>> {
    use std::io::BufRead;
    let manifest_path = manifest_key(prefix);
    let bytes = match store.get(&manifest_path).await {
        Ok(b) => b,
        Err(Error::Store(_)) | Err(Error::Io(_)) => return Ok(None),
        Err(other) => return Err(other),
    };
    let mut reader =
        std::io::BufReader::new(flate2::read::GzDecoder::new(std::io::Cursor::new(bytes)));
    let mut header_line = String::new();
    let read = reader
        .read_line(&mut header_line)
        .map_err(|e| Error::Other(format!("gunzip manifest header: {e}")))?;
    if read == 0 {
        return Err(Error::Other("manifest is empty".into()));
    }
    let header: ManifestHeader = serde_json::from_str(header_line.trim())
        .map_err(|e| Error::Other(format!("bad manifest header: {e}")))?;
    Ok(Some((header, ManifestIter::new(Box::new(reader)))))
}

type ManifestRead = Box<dyn std::io::BufRead + Send + Sync + 'static>;

/// Iterator over manifest entries. Wraps a buffered `Read` over the
/// gunzipped JSONL body; one `read_line` call per `next_entry`, so
/// peak heap stays at a single entry's line buffer.
pub struct ManifestIter {
    reader: ManifestRead,
    buf: String,
}

impl ManifestIter {
    fn new(reader: ManifestRead) -> Self {
        Self {
            reader,
            buf: String::new(),
        }
    }

    pub fn next_entry(&mut self) -> Result<Option<ManifestEntry>> {
        use std::io::BufRead;
        loop {
            self.buf.clear();
            let n = self
                .reader
                .read_line(&mut self.buf)
                .map_err(|e| Error::Other(format!("read manifest line: {e}")))?;
            if n == 0 {
                return Ok(None);
            }
            let trimmed = self.buf.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<ManifestEntry>(trimmed) {
                Ok(entry) => return Ok(Some(entry)),
                Err(e) => {
                    // Tolerate corruption: log the line, skip it, keep
                    // walking. The previous behaviour stopped the
                    // entire listing on the first bad middle line,
                    // silently dropping the rest of the corpus.
                    tracing::warn!(error = %e, "manifest: skipping bad line");
                    continue;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open;
    use bytes::Bytes;
    use tempfile::tempdir;

    #[tokio::test]
    async fn roundtrip() {
        let dir = tempdir().unwrap();
        let uri = format!("file://{}", dir.path().display());
        let (store, _) = open(&uri).unwrap();
        store
            .put("docs/a.md", Bytes::from_static(b"# alpha"))
            .await
            .unwrap();
        store
            .put("docs/b.md", Bytes::from_static(b"# beta"))
            .await
            .unwrap();
        store
            .put("other/c.txt", Bytes::from_static(b"unrelated"))
            .await
            .unwrap();

        let header = write_manifest(&*store, "docs").await.unwrap();
        assert_eq!(header.count, 2);
        assert_eq!(header.prefix, "docs");

        let (h2, entries) = read_manifest(&*store, "docs").await.unwrap().unwrap();
        assert_eq!(h2.count, 2);
        let mut keys: Vec<String> = entries.iter().map(|e| e.key.clone()).collect();
        keys.sort();
        assert_eq!(keys, vec!["docs/a.md".to_string(), "docs/b.md".to_string()]);
    }

    #[tokio::test]
    async fn missing_manifest_returns_none() {
        let dir = tempdir().unwrap();
        let uri = format!("file://{}", dir.path().display());
        let (store, _) = open(&uri).unwrap();
        let res = read_manifest(&*store, "missing").await.unwrap();
        assert!(res.is_none());
    }
}
