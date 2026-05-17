use as_fs::Fs;
use as_lex::grep::{grep_bytes, GrepOpts};
use clap::{Parser, Subcommand};
use futures::stream::StreamExt;

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
        /// Optional glob pattern to filter listed keys.
        #[arg(long)]
        glob: Option<String>,
        /// Limit the number of entries printed.
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
    /// Run a ripgrep-style search over an object-store prefix.
    Grep {
        uri: String,
        pattern: String,
        #[arg(short = 'i', long)]
        case_insensitive: bool,
        /// Stop scanning after N matches across the whole prefix.
        #[arg(long, default_value_t = 1000)]
        max_hits: usize,
    },
    /// Build a tantivy index over a prefix (M2+).
    Index {
        uri: String,
        #[arg(long, default_value = ".agentic-search/index")]
        out: String,
    },
    /// Run a query against an index (M2+).
    Query {
        index: String,
        query: String,
        #[arg(short = 'k', long, default_value_t = 10)]
        k: usize,
    },
    /// Serve the HTTP API.
    Serve {
        #[arg(long, default_value = "0.0.0.0:8787")]
        bind: String,
    },
    /// Run benchmarks (M6+).
    Bench {
        #[arg(long)]
        suite: Option<String>,
    },
}

/// Open a store from a URI like `s3://bucket/prefix` and return
/// `(fs, key_prefix)` ready for `ls` / `read` / `grep`.
fn open_fs(uri: &str) -> anyhow::Result<(Fs, String)> {
    let (store, prefix) = as_store::open(uri)?;
    Ok((Fs::new(store), prefix))
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
) -> anyhow::Result<()> {
    let (fs, prefix) = open_fs(&uri)?;
    let opts = GrepOpts {
        case_insensitive,
        multi_line: false,
        max_hits_per_file: None,
    };
    let mut stream = fs.list(&prefix);
    let mut total = 0usize;
    while let Some(item) = stream.next().await {
        let meta = item?;
        // Skip obviously-binary or huge files to keep the cold path responsive.
        if meta.size > 64 * 1024 * 1024 {
            continue;
        }
        let bytes = match fs.read(&meta.key).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(key = %meta.key, error = %e, "read failed, skipping");
                continue;
            }
        };
        match grep_bytes(&meta.key, &bytes, &pattern, &opts) {
            Ok(hits) => {
                for h in hits {
                    let line = h.metadata.get("line").and_then(|v| v.as_u64()).unwrap_or(0);
                    let snippet = h.snippet.unwrap_or_default();
                    println!("{}:{}: {}", h.uri, line, snippet);
                    total += 1;
                    if total >= max_hits {
                        return Ok(());
                    }
                }
            }
            Err(e) => tracing::warn!(key = %meta.key, error = %e, "grep failed"),
        }
    }
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
        } => cmd_grep(uri, pattern, case_insensitive, max_hits).await?,
        Cmd::Index { uri, out } => {
            println!("index (M2+) {uri} -> {out}");
        }
        Cmd::Query { index, query, k } => {
            println!("query (M2+) {index} q={query:?} k={k}");
        }
        Cmd::Serve { bind } => {
            let app = as_server::router();
            let listener = tokio::net::TcpListener::bind(&bind).await?;
            tracing::info!(%bind, "serving");
            axum::serve(listener, app).await?;
        }
        Cmd::Bench { suite } => {
            println!("bench (M6+) suite={suite:?}");
        }
    }
    Ok(())
}
