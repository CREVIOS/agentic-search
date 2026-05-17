//! SIFT-1M benchmark for the `as-vec` centroid index.
//!
//! Loads the standard SIFT-1M dataset (1M × 128d base vectors, 10k
//! queries, ground-truth top-100 per query) from
//! `bench/data/sift/`, trains a centroid index in-process, persists
//! it to a local directory under the `as-vec` on-disk format, then
//! runs the query path against the 10k queries and reports:
//!
//!   * index build wall time
//!   * index size on disk
//!   * query p50 / p95 / p99 latency at chosen `probe`
//!   * recall@10 vs ground-truth top-10
//!
//! Direct comparison against Turbopuffer's published numbers
//! (1M cold p50 343 ms, warm p50 8 ms, 90-95% recall@10) and any
//! other public S3-shaped centroid index.
//!
//! Run:
//!   cargo run --release -p bench --bin sift1m -- --probe 32 --k 10

use anyhow::{anyhow, Context, Result};
use as_store::open;
use as_vec::index::{encode_centroids, encode_cluster, ClusterRecord, DocMeta};
use as_vec::kmeans::{normalize, train};
use as_vec::manifest::{Manifest, MANIFEST_VERSION};
use as_vec::query::VecIndex;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

const DIM: usize = 128;

fn main() -> Result<()> {
    let mut args: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let key = k.trim_start_matches("--").to_string();
        let v = it.next().unwrap_or_default();
        args.insert(key, v);
    }
    let data = args
        .get("data")
        .cloned()
        .unwrap_or_else(|| "bench/data/sift".into());
    let index_dir = args
        .get("index")
        .cloned()
        .unwrap_or_else(|| "bench/data/sift-index".into());
    let k_clusters: usize = args
        .get("k-clusters")
        .map(|s| s.parse().unwrap())
        .unwrap_or(1024);
    let iters: usize = args
        .get("iters")
        .map(|s| s.parse().unwrap())
        .unwrap_or(15);
    let top_k: usize = args
        .get("k")
        .map(|s| s.parse().unwrap())
        .unwrap_or(10);
    let probe: usize = args
        .get("probe")
        .map(|s| s.parse().unwrap())
        .unwrap_or(32);
    let n_queries: usize = args
        .get("queries")
        .map(|s| s.parse().unwrap())
        .unwrap_or(1000);
    let rebuild: bool = args
        .get("rebuild")
        .map(|s| s != "0" && s != "false")
        .unwrap_or(false);

    let data = PathBuf::from(data);
    let index_dir = PathBuf::from(index_dir);

    println!("SIFT-1M bench");
    println!(
        "  data={}  index={}  k-clusters={}  iters={}  probe={}  k={}  queries={}",
        data.display(),
        index_dir.display(),
        k_clusters,
        iters,
        probe,
        top_k,
        n_queries,
    );

    if rebuild || !index_dir.join("manifest.json").exists() {
        build_index(&data, &index_dir, k_clusters, iters)?;
    } else {
        println!("[skip build] using existing index at {}", index_dir.display());
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run_queries(
        &data, &index_dir, top_k, probe, n_queries,
    ))?;
    Ok(())
}

fn build_index(data: &Path, out: &Path, k_clusters: usize, iters: usize) -> Result<()> {
    println!("== build ==");
    let t0 = Instant::now();
    let mut vectors = read_fvecs(&data.join("sift_base.fvecs"), DIM)?;
    println!(
        "  read {} base vectors ({:.1}s)",
        vectors.len(),
        t0.elapsed().as_secs_f64()
    );

    let t1 = Instant::now();
    for v in vectors.iter_mut() {
        normalize(v);
    }
    println!("  normalized base ({:.2}s)", t1.elapsed().as_secs_f64());

    let t2 = Instant::now();
    let (centroids, assignments) = train(&vectors, k_clusters, iters)
        .map_err(|e| anyhow!("kmeans: {e}"))?;
    println!("  trained kmeans ({:.1}s)", t2.elapsed().as_secs_f64());

    let t3 = Instant::now();
    std::fs::create_dir_all(out).context("create index dir")?;
    let mut buckets: Vec<Vec<ClusterRecord>> = (0..k_clusters).map(|_| Vec::new()).collect();
    for (i, (v, &cid)) in vectors.into_iter().zip(assignments.iter()).enumerate() {
        buckets[cid as usize].push(ClusterRecord {
            doc_id: i as u32,
            vector: v,
        });
    }
    let centroid_bytes = encode_centroids(&centroids);
    std::fs::write(out.join("centroids.f32"), &centroid_bytes)?;
    let manifest_obj = Manifest::new(DIM, k_clusters, as_embed_default());
    let mut sizes = vec![0u32; k_clusters];
    for (cid, bucket) in buckets.iter().enumerate() {
        sizes[cid] = bucket.len() as u32;
        let cf = &manifest_obj.cluster_files[cid];
        if bucket.is_empty() {
            std::fs::write(out.join(cf), b"")?;
            continue;
        }
        let bytes = encode_cluster(bucket, DIM);
        std::fs::write(out.join(cf), bytes)?;
    }
    // docs.jsonl — synthetic metadata, just doc_id -> uri.
    let mut docs = File::create(out.join("docs.jsonl"))?;
    for i in 0..sizes.iter().map(|s| *s as usize).sum::<usize>() {
        let m = DocMeta {
            id: i as u32,
            uri: format!("sift://1m/{i:07}"),
            byte_range: [0, 0],
            snippet: String::new(),
        };
        let line = serde_json::to_string(&m)?;
        writeln!(docs, "{line}")?;
    }
    let manifest = Manifest {
        version: MANIFEST_VERSION,
        dim: DIM,
        k: k_clusters,
        embed_model: manifest_obj.embed_model,
        num_docs: sizes.iter().map(|s| *s as u64).sum::<u64>(),
        cluster_sizes: sizes,
        cluster_files: manifest_obj.cluster_files,
        centroids_file: manifest_obj.centroids_file,
        docs_file: manifest_obj.docs_file,
        chunk_chars: 0,
        chunk_overlap: 0,
    };
    std::fs::write(out.join("manifest.json"), serde_json::to_vec_pretty(&manifest)?)?;
    println!("  wrote index ({:.1}s)", t3.elapsed().as_secs_f64());

    let total_bytes: u64 = walk_dir_size(out)?;
    println!(
        "  index size on disk: {:.1} MB",
        total_bytes as f64 / 1e6
    );

    println!("== build complete in {:.1}s ==", t0.elapsed().as_secs_f64());
    Ok(())
}

async fn run_queries(
    data: &Path,
    index_dir: &Path,
    top_k: usize,
    probe: usize,
    n_queries: usize,
) -> Result<()> {
    println!("== query ==");
    let queries = read_fvecs(&data.join("sift_query.fvecs"), DIM)?;
    let gt = read_ivecs(&data.join("sift_groundtruth.ivecs"))?;
    println!("  loaded {} queries, {} gt rows", queries.len(), gt.len());

    let uri = format!("file://{}", index_dir.canonicalize()?.display());
    let (store, prefix) = open(&uri)?;
    let index = VecIndex::open(store, &prefix)
        .await
        .map_err(|e| anyhow!("open vec index: {e}"))?;

    // Warm-up: one query so OS page cache + LRUs catch a fresh state.
    let mut q0 = queries[0].clone();
    normalize(&mut q0);
    let _ = index
        .query_vec(&q0, top_k, probe)
        .await
        .map_err(|e| anyhow!("warmup: {e}"))?;

    let n = n_queries.min(queries.len());
    let mut latencies_us: Vec<u64> = Vec::with_capacity(n);
    let mut recall_sum = 0.0f64;
    for i in 0..n {
        let mut q = queries[i].clone();
        normalize(&mut q);
        let t = Instant::now();
        let hits = index
            .query_vec(&q, top_k, probe)
            .await
            .map_err(|e| anyhow!("query {i}: {e}"))?;
        latencies_us.push(t.elapsed().as_micros() as u64);

        // Recall@k vs ground truth top-k.
        let gt_set: std::collections::HashSet<u32> =
            gt[i].iter().take(top_k).copied().collect();
        let hits_set: std::collections::HashSet<u32> =
            hits.iter().map(|h| h.doc.id).collect();
        let hit_count = gt_set.intersection(&hits_set).count();
        recall_sum += hit_count as f64 / top_k as f64;
    }

    latencies_us.sort_unstable();
    let p = |q: f64| -> f64 {
        let i = ((latencies_us.len() as f64 - 1.0) * q).round() as usize;
        latencies_us[i] as f64 / 1000.0
    };
    let mean_ms =
        latencies_us.iter().map(|x| *x as f64).sum::<f64>() / latencies_us.len() as f64 / 1000.0;
    let recall = recall_sum / n as f64;
    println!(
        "== results ({} queries, probe={}, k={}) ==",
        n, probe, top_k
    );
    println!("  recall@{}      : {:.4}", top_k, recall);
    println!("  latency mean   : {:.2} ms", mean_ms);
    println!("  latency p50    : {:.2} ms", p(0.50));
    println!("  latency p95    : {:.2} ms", p(0.95));
    println!("  latency p99    : {:.2} ms", p(0.99));
    println!(
        "  throughput     : {:.0} qps (single-thread)",
        1000.0 / mean_ms
    );
    Ok(())
}

fn read_fvecs(path: &Path, expected_dim: usize) -> Result<Vec<Vec<f32>>> {
    let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut r = BufReader::with_capacity(8 * 1024 * 1024, f);
    let mut out = Vec::new();
    let mut buf4 = [0u8; 4];
    loop {
        match r.read_exact(&mut buf4) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
        let dim = u32::from_le_bytes(buf4) as usize;
        if dim != expected_dim {
            return Err(anyhow!(
                "fvecs dim {dim} != expected {expected_dim} in {}",
                path.display()
            ));
        }
        let mut v = vec![0f32; dim];
        let mut bytes = vec![0u8; dim * 4];
        r.read_exact(&mut bytes)?;
        for j in 0..dim {
            v[j] = f32::from_le_bytes(bytes[j * 4..(j + 1) * 4].try_into().unwrap());
        }
        out.push(v);
    }
    Ok(out)
}

fn read_ivecs(path: &Path) -> Result<Vec<Vec<u32>>> {
    let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut r = BufReader::with_capacity(8 * 1024 * 1024, f);
    let mut out = Vec::new();
    let mut buf4 = [0u8; 4];
    loop {
        match r.read_exact(&mut buf4) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
        let dim = u32::from_le_bytes(buf4) as usize;
        let mut v = vec![0u32; dim];
        let mut bytes = vec![0u8; dim * 4];
        r.read_exact(&mut bytes)?;
        for j in 0..dim {
            v[j] = u32::from_le_bytes(bytes[j * 4..(j + 1) * 4].try_into().unwrap());
        }
        out.push(v);
    }
    Ok(out)
}

fn walk_dir_size(dir: &Path) -> Result<u64> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(dir)? {
        let e = entry?;
        let m = e.metadata()?;
        if m.is_file() {
            total += m.len();
        } else if m.is_dir() {
            total += walk_dir_size(&e.path())?;
        }
    }
    Ok(total)
}

fn as_embed_default() -> as_embed::Model {
    as_embed::Model::BgeSmallEnV15
}
