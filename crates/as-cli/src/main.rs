use as_ast::{widen_with_cache, SpanCache};
use as_fs::Fs;
use as_grep::{grep_bytes_spans, GrepOpts, ParallelGrep, ParallelOpts, Span};
use clap::{Parser, Subcommand};
use futures::stream::StreamExt;
use std::sync::Arc;

#[derive(Parser)]
#[command(
    name = "agentic-search",
    version,
    about = "Fastest agentic search over S3 + web"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List objects under a URI prefix (s3://, gs://, file://).
    Ls {
        uri: String,
        #[arg(long)]
        glob: Option<String>,
        #[arg(long, default_value_t = 1000)]
        limit: usize,
    },
    /// Read an object (optional byte range) to stdout.
    Read {
        uri: String,
        #[arg(long)]
        offset: Option<u64>,
        #[arg(long)]
        length: Option<u64>,
    },
    /// Ripgrep-style search over an object-store prefix, in parallel.
    Grep {
        uri: String,
        pattern: String,
        #[arg(short = 'i', long)]
        case_insensitive: bool,
        #[arg(long, default_value_t = 1000)]
        max_hits: usize,
        #[arg(long, default_value_t = 32)]
        concurrency: usize,
        /// Widen each hit to its enclosing function/class/method via tree-sitter.
        #[arg(long)]
        ast: bool,
    },
    /// Find symbols (function/class/method) by name across a prefix.
    Find {
        uri: String,
        #[arg(long)]
        symbol: String,
        #[arg(long, default_value_t = 32)]
        concurrency: usize,
        #[arg(long, default_value_t = 200)]
        max_hits: usize,
    },
    /// Build a Turbopuffer-style centroid vector index over a corpus
    /// prefix and write it under `<corpus>/.agentic-search/index/<ns>/`.
    Index {
        /// Corpus URI (e.g. `s3://my-corpus/docs/` or `file:///path`).
        uri: String,
        /// Namespace name; the index lives at
        /// `<uri>/.agentic-search/index/<namespace>/`.
        #[arg(long, default_value = "default")]
        namespace: String,
        /// Maximum file size to index, in bytes.
        #[arg(long, default_value_t = 4 * 1024 * 1024)]
        max_file_bytes: u64,
        /// Cluster count (`K`). Defaults to `sqrt(num_chunks)`.
        #[arg(long)]
        k: Option<usize>,
        /// Glob pattern (relative to `uri`) selecting files to index.
        #[arg(long, default_value = "**/*.md")]
        glob: String,
    },
    /// Run a hybrid grep + vector search against an indexed corpus.
    Query {
        /// Corpus URI (must match the URI used at index time).
        uri: String,
        /// Natural-language query.
        query: String,
        #[arg(long, default_value = "default")]
        namespace: String,
        #[arg(short = 'k', long, default_value_t = 10)]
        k: usize,
        /// Number of clusters to inspect (Turbopuffer-style `probe`).
        #[arg(long, default_value_t = 8)]
        probe: usize,
    },
    /// Serve the HTTP + MCP API.
    Serve {
        #[arg(long, default_value = "0.0.0.0:8787")]
        bind: String,
        /// Speak MCP on stdio instead of HTTP.
        #[arg(long)]
        mcp: bool,
    },
    /// Run benchmarks (M6+).
    Bench {
        #[arg(long)]
        suite: Option<String>,
    },
    /// Build a prefix manifest so cold `list` is one GET instead of
    /// paged ListObjectsV2. Manifest is written to
    /// `<uri>/.agentic-search/manifest.jsonl.gz`.
    IndexManifest { uri: String },
}

fn open_fs(uri: &str) -> anyhow::Result<(Arc<Fs>, String)> {
    let (store, prefix) = as_store::open(uri)?;
    // Wrap remote stores in the tier cache so warm S3 reads stay near the
    // CPU. Local `file://` doesn't benefit (OS page cache already hot), so
    // we skip the wrap to avoid an extra hop.
    let store = if uri.starts_with("file://") {
        store
    } else {
        as_cache::wrap(store, as_cache::TierConfig::default())
    };
    Ok((Arc::new(Fs::new(store)), prefix))
}

async fn cmd_ls(uri: String, glob: Option<String>, limit: usize) -> anyhow::Result<()> {
    let (fs, prefix) = open_fs(&uri)?;
    let mut stream = match glob.as_deref() {
        Some(pat) => fs.glob(&prefix, pat)?,
        None => fs.list(&prefix),
    };
    let mut printed = 0usize;
    while let Some(item) = stream.next().await {
        let m = item?;
        println!("{:>12}  {}", m.size, m.key);
        printed += 1;
        if printed >= limit {
            break;
        }
    }
    Ok(())
}

async fn cmd_read(uri: String, offset: Option<u64>, length: Option<u64>) -> anyhow::Result<()> {
    let (fs, key) = open_fs(&uri)?;
    use std::io::Write;
    let data = match (offset, length) {
        (Some(o), Some(l)) => fs.read_at(&key, o..o.saturating_add(l)).await?,
        (None, None) => fs.read(&key).await?,
        _ => anyhow::bail!("--offset and --length must be provided together"),
    };
    std::io::stdout().write_all(&data)?;
    Ok(())
}

async fn cmd_grep(
    uri: String,
    pattern: String,
    case_insensitive: bool,
    max_hits: usize,
    concurrency: usize,
    ast: bool,
) -> anyhow::Result<()> {
    let (fs, prefix) = open_fs(&uri)?;
    let opts = ParallelOpts {
        grep: GrepOpts {
            case_insensitive,
            multi_line: false,
            max_hits_per_file: None,
        },
        concurrency,
        max_object_bytes: 64 * 1024 * 1024,
        max_total_spans: Some(max_hits),
    };
    let pg = ParallelGrep::new(fs.clone());
    let spans = pg.scan_prefix(&prefix, &pattern, &opts).await?;
    let printed = if ast {
        widen_spans(&fs, spans).await?
    } else {
        spans
    };
    print_spans(&printed);
    Ok(())
}

async fn cmd_find(
    uri: String,
    symbol: String,
    concurrency: usize,
    max_hits: usize,
) -> anyhow::Result<()> {
    // `find_symbol` is implemented as a regex grep of an identifier-ish
    // boundary around `symbol`, followed by AST widening. This avoids a
    // language-specific parser walk over every byte of every file.
    let (fs, prefix) = open_fs(&uri)?;
    let pattern = format!(r"\b{}\b", regex_escape(&symbol));
    let opts = ParallelOpts {
        grep: GrepOpts {
            case_insensitive: false,
            multi_line: false,
            max_hits_per_file: None,
        },
        concurrency,
        max_object_bytes: 64 * 1024 * 1024,
        max_total_spans: Some(max_hits.saturating_mul(4)),
    };
    let pg = ParallelGrep::new(fs.clone());
    let spans = pg.scan_prefix(&prefix, &pattern, &opts).await?;
    let mut spans = widen_spans(&fs, spans).await?;
    spans.retain(|s| s.symbol.as_deref() == Some(symbol.as_str()));
    spans.truncate(max_hits);
    print_spans(&spans);
    Ok(())
}

async fn widen_spans(fs: &Arc<Fs>, spans: Vec<Span>) -> anyhow::Result<Vec<Span>> {
    use std::collections::{BTreeMap, HashSet};
    let mut by_uri: BTreeMap<String, Vec<Span>> = BTreeMap::new();
    for s in spans {
        by_uri.entry(s.uri.clone()).or_default().push(s);
    }
    let mut out: Vec<Span> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    // One cache per CLI invocation; we don't yet persist it across runs.
    let cache = SpanCache::default();
    for (uri, mut group) in by_uri {
        let bytes = match fs.read(&uri).await {
            Ok(b) => b,
            Err(_) => continue,
        };
        widen_with_cache(&cache, &bytes, &mut group)?;
        for s in group {
            if seen.insert(s.dedup_key()) {
                out.push(s);
            }
        }
    }
    out.sort_by(|a, b| {
        a.uri
            .cmp(&b.uri)
            .then(a.line_range[0].cmp(&b.line_range[0]))
            .then(a.byte_range.start.cmp(&b.byte_range.start))
    });
    Ok(out)
}

fn print_spans(spans: &[Span]) {
    for s in spans {
        let sym = s.symbol.as_deref().unwrap_or("");
        let snippet_line = s
            .snippet
            .as_deref()
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("");
        println!(
            "{}:{}-{} [{:?}{}{}]\t{}",
            s.uri,
            s.line_range[0],
            s.line_range[1],
            s.kind,
            if sym.is_empty() { "" } else { " " },
            sym,
            snippet_line
        );
    }
}

fn regex_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(
            c,
            '.' | '+'
                | '*'
                | '?'
                | '('
                | ')'
                | '|'
                | '['
                | ']'
                | '{'
                | '}'
                | '^'
                | '$'
                | '\\'
                | '/'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

async fn cmd_index(
    uri: String,
    namespace: String,
    max_file_bytes: u64,
    k: Option<usize>,
    glob: String,
) -> anyhow::Result<()> {
    use as_embed::Model;
    use as_vec::build::{build_index, chunk_text, InputDoc};
    use futures::stream::StreamExt;

    let (fs, prefix) = open_fs(&uri)?;
    let (store, _) = as_store::open(&uri)?;

    let glob_pat = glob.clone();
    let mut listing = fs.glob(&prefix, &glob_pat)?;
    let mut docs: Vec<InputDoc> = Vec::new();
    let mut files_seen = 0usize;
    while let Some(item) = listing.next().await {
        let meta = item?;
        files_seen += 1;
        if meta.size == 0 || meta.size > max_file_bytes {
            continue;
        }
        let bytes = match fs.read_fresh(&meta).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(key = %meta.key, error = %e, "read failed, skipping");
                continue;
            }
        };
        let text = match std::str::from_utf8(&bytes) {
            Ok(s) => s.to_string(),
            Err(_) => continue, // not utf-8
        };
        for (start, end, chunk) in chunk_text(&text, 1200, 200) {
            docs.push(InputDoc {
                uri: meta.key.clone(),
                byte_range: [start, end],
                text: chunk,
            });
        }
    }
    if docs.is_empty() {
        anyhow::bail!(
            "no indexable files matched glob {glob_pat:?} under {uri} (scanned {files_seen} entries)"
        );
    }
    let ns_prefix = format!(
        "{}.agentic-search/index/{}",
        if prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", prefix.trim_end_matches('/'))
        },
        namespace
    );
    let manifest = build_index(store, &ns_prefix, docs, Model::default(), k, 8).await?;
    println!(
        "indexed {} chunks ({} files scanned) into {}/{}/  K={} dim={}",
        manifest.num_docs,
        files_seen,
        uri.trim_end_matches('/'),
        ns_prefix,
        manifest.k,
        manifest.dim
    );
    Ok(())
}

async fn cmd_index_manifest(uri: String) -> anyhow::Result<()> {
    let (store, prefix) = as_store::open(&uri)?;
    let header = as_store::manifest::write_manifest(&*store, &prefix).await?;
    println!(
        "manifest: {count} entries under {prefix:?} -> {uri}/.agentic-search/manifest.jsonl.gz",
        count = header.count,
        prefix = header.prefix,
    );
    Ok(())
}

async fn cmd_query(
    uri: String,
    namespace: String,
    query: String,
    k: usize,
    probe: usize,
) -> anyhow::Result<()> {
    use as_embed::{Embedder, Model};
    use as_plan::{PlanInputs, Planner};
    use as_vec::query::VecIndex;

    let (fs, prefix) = open_fs(&uri)?;
    let (store, _) = as_store::open(&uri)?;
    let ns_prefix = format!(
        "{}.agentic-search/index/{}",
        if prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", prefix.trim_end_matches('/'))
        },
        namespace
    );

    let vec_index = match VecIndex::open(store, &ns_prefix).await {
        Ok(i) => Some(i),
        Err(e) => {
            tracing::warn!(error = %e, "no vector index at {ns_prefix} — running grep-only");
            None
        }
    };
    let _embedder_guard: Option<Embedder> = vec_index.as_ref().and_then(|_| {
        // Pre-warm embedder so the first query in this run doesn't pay
        // for cold model init at the planner boundary.
        Embedder::new(Model::default()).ok()
    });

    let plan = PlanInputs {
        fs: fs.clone(),
        grep_prefix: &prefix,
        query: &query,
        k,
        grep_max_hits: k.saturating_mul(4).clamp(32, 512),
        grep_concurrency: 32,
        vec_index: vec_index.as_ref(),
        vec_probe: probe,
        vec_store: None,
    };
    let spans = Planner::search(plan).await?;
    print_spans(&spans);
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Ls { uri, glob, limit } => cmd_ls(uri, glob, limit).await?,
        Cmd::Read {
            uri,
            offset,
            length,
        } => cmd_read(uri, offset, length).await?,
        Cmd::Grep {
            uri,
            pattern,
            case_insensitive,
            max_hits,
            concurrency,
            ast,
        } => cmd_grep(uri, pattern, case_insensitive, max_hits, concurrency, ast).await?,
        Cmd::Find {
            uri,
            symbol,
            concurrency,
            max_hits,
        } => cmd_find(uri, symbol, concurrency, max_hits).await?,
        Cmd::Index {
            uri,
            namespace,
            max_file_bytes,
            k,
            glob,
        } => cmd_index(uri, namespace, max_file_bytes, k, glob).await?,
        Cmd::Query {
            uri,
            query,
            namespace,
            k,
            probe,
        } => cmd_query(uri, namespace, query, k, probe).await?,
        Cmd::Serve { bind, mcp } => {
            if mcp {
                as_server::mcp_stdio::run().await?;
            } else {
                let app = as_server::router();
                let listener = tokio::net::TcpListener::bind(&bind).await?;
                tracing::info!(%bind, "serving");
                axum::serve(listener, app).await?;
            }
        }
        Cmd::Bench { suite } => {
            println!("bench (M6) suite={suite:?}");
        }
        Cmd::IndexManifest { uri } => cmd_index_manifest(uri).await?,
    }
    Ok(())
}

// Re-export so the old `grep_bytes` and `grep_bytes_spans` names compile if any
// internal caller still uses them; the CLI itself uses `grep_bytes_spans`
// directly via `ParallelGrep`.
#[allow(dead_code)]
fn _ensure_grep_helpers(uri: &str, bytes: &[u8], pat: &str) {
    let _ = grep_bytes_spans(uri, bytes, pat, &GrepOpts::default());
}
