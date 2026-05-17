use as_fs::Fs;
use bytes::Bytes;
use futures::stream::TryStreamExt;
use tempfile::tempdir;

#[tokio::test]
async fn glob_filters_by_pattern() {
    let dir = tempdir().unwrap();
    let uri = format!("file://{}", dir.path().display());
    let (store, _) = as_store::open(&uri).unwrap();
    let fs = Fs::new(store);

    fs.write("docs/a.md", Bytes::from_static(b"a"))
        .await
        .unwrap();
    fs.write("docs/b.md", Bytes::from_static(b"b"))
        .await
        .unwrap();
    fs.write("docs/c.txt", Bytes::from_static(b"c"))
        .await
        .unwrap();

    let mut keys: Vec<String> = fs
        .glob("docs", "*.md")
        .unwrap()
        .map_ok(|m| m.key)
        .try_collect()
        .await
        .unwrap();
    keys.sort();
    assert_eq!(keys, vec!["docs/a.md".to_string(), "docs/b.md".to_string()]);
}
