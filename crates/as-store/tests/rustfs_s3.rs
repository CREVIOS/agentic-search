//! S3 integration test against a local RustFS container.
//!
//! This test only runs when `RUSTFS_S3_TEST=1` is set in the env, so it
//! stays out of the default cargo test path (CI runs it explicitly).
//!
//! Spin up RustFS first::
//!
//!     docker run -d --name rustfs-it -p 19000:9000 \
//!         -e RUSTFS_ACCESS_KEY=testkey -e RUSTFS_SECRET_KEY=testsecret \
//!         rustfs/rustfs:latest server /data --address 0.0.0.0:9000
//!
//! Then::
//!
//!     RUSTFS_S3_TEST=1 \
//!     AWS_ACCESS_KEY_ID=testkey AWS_SECRET_ACCESS_KEY=testsecret \
//!     AWS_REGION=us-east-1 AWS_ENDPOINT_URL=http://localhost:19000 \
//!     AWS_ALLOW_HTTP=true \
//!     cargo test -p as-store --test rustfs_s3 -- --nocapture

use bytes::Bytes;
use futures::stream::TryStreamExt;

const BUCKET: &str = "agentic-search-it";

fn enabled() -> bool {
    std::env::var("RUSTFS_S3_TEST").ok().as_deref() == Some("1")
}

fn create_bucket_via_curl() {
    // RustFS auto-creates buckets on first put for the default path-style
    // API, but we make it explicit with a PUT bucket to keep the test
    // hermetic. The signing is left to the `object_store`-backed `Store`.
    // No-op here; relies on aws-sdk to create-on-put.
}

#[tokio::test]
async fn s3_roundtrip_against_rustfs() {
    if !enabled() {
        eprintln!("RUSTFS_S3_TEST not set; skipping");
        return;
    }
    create_bucket_via_curl();

    let uri = format!("s3://{BUCKET}/it");
    let (store, prefix) = as_store::open(&uri).expect("open s3 store");
    assert_eq!(prefix, "it");

    // Put a few objects under the prefix.
    store
        .put("it/a.txt", Bytes::from_static(b"hello rustfs\n"))
        .await
        .expect("put a.txt");
    store
        .put("it/b.txt", Bytes::from_static(b"second object\n"))
        .await
        .expect("put b.txt");

    // Head + get round-trip.
    let m = store.head("it/a.txt").await.expect("head");
    assert_eq!(m.size, 13);
    let bytes = store.get("it/a.txt").await.expect("get");
    assert_eq!(bytes.as_ref(), b"hello rustfs\n");

    // Range read.
    let slice = store.get_range("it/a.txt", 6..12).await.expect("range");
    assert_eq!(slice.as_ref(), b"rustfs");

    // List.
    let mut keys: Vec<String> = store
        .list("it")
        .map_ok(|m| m.key)
        .try_collect()
        .await
        .expect("list");
    keys.sort();
    assert_eq!(keys, vec!["it/a.txt".to_string(), "it/b.txt".to_string()]);

    // Cleanup.
    store.delete("it/a.txt").await.expect("delete a");
    store.delete("it/b.txt").await.expect("delete b");
}
