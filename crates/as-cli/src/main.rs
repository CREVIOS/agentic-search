use as_ast::widen_many;
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
    /// Build a tantivy index over a prefix (optional / non-code corpora).
    Index {
        uri: String,
        #[arg(long, default_value = ".agentic-search/index")]
        out: String,
    },
    /// Run a query against an index.
    Query {
        index: String,
        query: String,
        #[arg(short = 'k', long, default_value_t = 10)]
        k: usize,
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
    let mut printed: Vec<Span> = Vec::with_capacity(spans.len());
    if ast {
        // Group by URI so each file is parsed once for many spans.
        use std::collections::{HashMap, HashSet};
        let mut by_uri: HashMap<String, Vec<Span>> = HashMap::new();
        for s in spans {
            by_uri.entry(s.uri.clone()).or_default().push(s);
        }
        let mut seen: HashSet<String> = HashSet::new();
        for (uri, mut group) in by_uri {
            let bytes = match fs.read(&uri).await {
                Ok(b) => b,
                Err(_) => continue,
            };
            widen_many(&bytes, &mut group)?;
            for s in group {
                if seen.insert(s.dedup_key()) {
                    printed.push(s);
                }
            }
        }
    } else {
        printed.extend(spans);
    }
    for s in &printed {
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
    let pattern = format!(r"\b{}\b", regex_escape(&symbol));
    cmd_grep(uri, pattern, false, max_hits, concurrency, true).await
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
        Cmd::Index { uri, out } => {
            println!("index (use as-index crate directly for now) {uri} -> {out}");
        }
        Cmd::Query { index, query, k } => {
            println!("query (use as-index crate directly for now) {index} q={query:?} k={k}");
        }
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
