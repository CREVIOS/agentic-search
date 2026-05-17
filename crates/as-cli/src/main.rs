use clap::{Parser, Subcommand};

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
    Ls { uri: String },
    /// Run a ripgrep-style search over an object-store prefix.
    Grep {
        uri: String,
        pattern: String,
        #[arg(short = 'i', long)]
        case_insensitive: bool,
    },
    /// Build a tantivy index over a prefix.
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
    /// Serve the HTTP API.
    Serve {
        #[arg(long, default_value = "0.0.0.0:8787")]
        bind: String,
    },
    /// Run benchmarks.
    Bench {
        #[arg(long)]
        suite: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("info").init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Ls { uri } => {
            println!("ls (stub) {uri}");
        }
        Cmd::Grep {
            uri,
            pattern,
            case_insensitive,
        } => {
            println!("grep (stub) {uri} {pattern} ci={case_insensitive}");
        }
        Cmd::Index { uri, out } => {
            println!("index (stub) {uri} -> {out}");
        }
        Cmd::Query { index, query, k } => {
            println!("query (stub) {index} q={query:?} k={k}");
        }
        Cmd::Serve { bind } => {
            let app = as_server::router();
            let listener = tokio::net::TcpListener::bind(&bind).await?;
            tracing::info!(%bind, "serving");
            axum::serve(listener, app).await?;
        }
        Cmd::Bench { suite } => {
            println!("bench (stub) suite={suite:?}");
        }
    }
    Ok(())
}
