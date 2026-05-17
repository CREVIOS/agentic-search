//! Native Rust usage — no HTTP server, no MCP, no JSON.
//!
//! Embeds `agentic-search-*` crates directly into a Rust binary and
//! drives the parallel grep + AST pipeline in-process against
//! `s3://agentic-search-it/corpus` (RustFS local) or any `file://`
//! corpus. Real-world shape: a CLI tool, a build hook, a server that
//! wants search without standing up a sidecar.
//!
//! Run:
//!   source scripts/rustfs-env.sh   # AWS_* must be set if using s3://
//!   cd examples/native_rust
//!   cargo run --release -- s3://agentic-search-it/corpus "graceful shutdown"

use anyhow::{anyhow, Result};
use as_fs::Fs;
use as_grep::{GrepOpts, ParallelGrep, ParallelOpts};
use as_store::open;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let uri = args
        .next()
        .unwrap_or_else(|| "file://examples/corpus/data".to_string());
    let pattern = args.next().unwrap_or_else(|| "graceful shutdown".into());

    println!("== in-process grep ==");
    println!("  uri    : {uri}");
    println!("  pattern: {pattern}");

    let (store, prefix) = open(&uri).map_err(|e| anyhow!("open: {e}"))?;
    let fs = Arc::new(Fs::new(store));
    let pg = ParallelGrep::new(fs);

    let opts = ParallelOpts {
        grep: GrepOpts {
            case_insensitive: false,
            multi_line: false,
            max_hits_per_file: None,
            stamp_content_hash: false,
        },
        concurrency: 32,
        max_object_bytes: 64 * 1024 * 1024,
        max_total_spans: Some(5),
    };

    let t = std::time::Instant::now();
    let spans = pg
        .scan_prefix(&prefix, &pattern, &opts)
        .await
        .map_err(|e| anyhow!("scan: {e}"))?;
    let elapsed = t.elapsed();

    println!("\n== {} hits in {:.1?} ==", spans.len(), elapsed);
    for s in &spans {
        let snip = s.snippet.as_deref().unwrap_or("").trim();
        println!(
            "  {}:{}  {}",
            s.uri,
            s.line_range[0],
            &snip[..snip.len().min(80)]
        );
    }
    Ok(())
}
