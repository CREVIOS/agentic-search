//! End-to-end tests for `as-store::open` using a `file://` backend.
//!
//! Exercises the `Store` trait without requiring AWS credentials. The same
//! code path serves S3, GCS, and R2; the integration there is asserted in
//! M1's MinIO test (run manually until we wire docker into CI).

use as_store::open;
use bytes::Bytes;
use futures::stream::TryStreamExt;
use tempfile::tempdir;

#[tokio::test]
async fn put_get_head_list_roundtrip() {
    let dir = tempdir().unwrap();
    let uri = format!("file://{}", dir.path().display());
    let (store, prefix) = open(&uri).expect("open file store");
    assert_eq!(prefix, "");

    store
        .put("docs/readme.md", Bytes::from_static(b"# hello\nworld\n"))
        .await
        .expect("put");
    store
        .put("docs/api.md", Bytes::from_static(b"# api\nendpoint\n"))
        .await
        .expect("put");
    store
        .put("data/x.bin", Bytes::from_static(&[0u8; 16]))
        .await
        .expect("put");

    let meta = store.head("docs/readme.md").await.expect("head");
    assert_eq!(meta.size, 14);

    let bytes = store.get("docs/readme.md").await.expect("get");
    assert_eq!(bytes.as_ref(), b"# hello\nworld\n");

    let slice = store
        .get_range("docs/readme.md", 2..7)
        .await
        .expect("range");
    assert_eq!(slice.as_ref(), b"hello");

    let mut keys: Vec<String> = store
        .list("docs")
        .map_ok(|m| m.key)
        .try_collect()
        .await
        .expect("list");
    keys.sort();
    assert_eq!(
        keys,
        vec!["docs/api.md".to_string(), "docs/readme.md".to_string()]
    );
}

#[tokio::test]
async fn delete_removes_object() {
    let dir = tempdir().unwrap();
    let uri = format!("file://{}", dir.path().display());
    let (store, _) = open(&uri).unwrap();

    store
        .put("tmp/a.txt", Bytes::from_static(b"a"))
        .await
        .unwrap();
    assert!(store.head("tmp/a.txt").await.is_ok());
    store.delete("tmp/a.txt").await.unwrap();
    assert!(store.head("tmp/a.txt").await.is_err());
}
