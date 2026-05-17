# Examples

Four real, runnable examples driving `agentic-search` against a 4 MB
markdown corpus (Rust Book + Tokio tutorial + Kubernetes concepts,
297 files). All four hit a **real S3-compatible endpoint** via the
included RustFS container — same wire protocol as production AWS S3,
just pointed at `http://localhost:19000`.

| Example                                  | Language    | Surface          | Driver                       |
| ---------------------------------------- | ----------- | ---------------- | ---------------------------- |
| `claude_agent_corpus.py`                 | Python      | **MCP stdio**    | Claude Agent SDK (Opus 4.7)  |
| `deepagents_corpus.py`                   | Python      | **REST**         | DeepAgents (Sonnet 4.6)      |
| `node_corpus.ts`                         | TypeScript  | REST             | `@agentic-search/sdk`        |
| `go_corpus/main.go`                      | Go          | REST             | `agenticsearch` Go SDK       |
| `native_python_corpus.py`                | Python      | REST (no SDK)    | bare `agentic_search` client |
| `native_rust/main.rs`                    | Rust        | **in-process**   | crate API, no server         |

Captured transcripts of each live run are checked in under
`transcripts/`:

- **[big_run_2026-05-18.md](transcripts/big_run_2026-05-18.md) — the
  full 10,843-file, real-S3, all-7-surfaces, timed + accuracy-checked
  end-to-end report.** Read this one if you only read one.
- [claude_agent_sdk_run.md](transcripts/claude_agent_sdk_run.md) —
  earlier 4 MB corpus run; Claude Opus 4.7 + 9 MCP tool calls.
- [deepagents_run.md](transcripts/deepagents_run.md) — earlier 4 MB
  corpus run; DeepAgents/Sonnet 4.6 over REST.
- [node_and_go_run.md](transcripts/node_and_go_run.md) — Node + Go
  SDK byte-identical output check.

## Run them locally

### 1. Build the corpus (one-time, ~30 s)

```bash
bash examples/corpus/build.sh
```

Sparse-clones markdown from `rust-lang/book`, `tokio-rs/website`,
`kubernetes/website` into `examples/corpus/data/`.

### 2. Boot RustFS (S3-compatible local server)

```bash
bash scripts/rustfs-up.sh
source scripts/rustfs-env.sh        # exports AWS_* for this shell
aws --endpoint-url "$AWS_ENDPOINT_URL" s3 sync \
    examples/corpus/data s3://agentic-search-it/corpus
```

297 files, ~3.5 MB, uploaded with signed PUTs.

### 3. Start the agentic-search server

```bash
cargo build --release -p agentic-search-cli
source scripts/rustfs-env.sh        # AWS_* must be present so the
                                    # server can sign S3 requests
target/release/agentic-search serve --bind 127.0.0.1:8787 &
curl -s http://127.0.0.1:8787/health   # → "ok"
```

The server binds **loopback by default**. Every `s3://…` request out
of it is a real SigV4-signed S3 API call against
`AWS_ENDPOINT_URL=http://localhost:19000` (RustFS). Swap the AWS
env for real AWS credentials and the same binary talks to real S3.

### 4. Run any of the four examples

```bash
# Claude Agent SDK over MCP stdio
python3 -m venv .venv-examples && source .venv-examples/bin/activate
pip install claude-agent-sdk requests
python examples/claude_agent_corpus.py

# DeepAgents over REST
pip install deepagents
pip install -e sdks/python/deepagents_search
python examples/deepagents_corpus.py

# Node SDK over REST
pnpm install
npx --yes tsx examples/node_corpus.ts

# Go SDK over REST
cd examples/go_corpus && go mod tidy && go run .

# --- native paths (no MCP, no agent framework) ---

# Python: just the REST client, no LLM, no agent SDK
pip install agentic-search
python examples/native_python_corpus.py

# Rust: in-process — no HTTP, no MCP, embeds agentic-search crates
cd examples/native_rust
cargo run --release -- s3://agentic-search-it/corpus "graceful shutdown"

# CLI: pipe-friendly grep against an s3:// prefix
target/release/agentic-search grep \
    s3://agentic-search-it/corpus 'graceful shutdown' --max-hits 5
```

## Security model — how this maps to production S3

The examples run on RustFS to keep the loop reproducible, but the
**security surface is identical to real S3**:

1. **Credentials never reach the agent.** The agent process only
   knows the agentic-search HTTP server (`127.0.0.1:8787`). The
   AWS env (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
   `AWS_SESSION_TOKEN`, optional `AWS_ENDPOINT_URL`) lives only in
   the server process. Compromising the agent's environment does
   not yield S3 keys.
2. **Server binds loopback by default.** `agentic-search serve`
   refuses to bind a non-loopback address without `--allow-public`,
   so a stray laptop daemon cannot be hit from the network.
3. **Every S3 request is signed.** The underlying `object_store`
   crate uses SigV4 against AWS / R2 / GCS / RustFS. Anonymous
   bucket reads are not supported on the hot path; revoking the
   server's IAM key revokes all downstream access immediately.
4. **Path-escape rejection.** `file://` paths still reject `..`
   segments at `LocalMmapStore::safe_path` for the local-dev path.
5. **Scope to read-only.** For production deploys, hand the server
   a read-only IAM role (`s3:GetObject`, `s3:ListBucket` on the
   target prefix only). Indexing / manifest writes need a separate
   role used only by the `agentic-search index*` commands.
6. **TLS.** Real S3 uses HTTPS by default. `AWS_ALLOW_HTTP=true` is
   set only for the local RustFS demo; remove it for prod so
   downgrade attacks fail closed.

`docker-compose.rustfs.yml` keeps the RustFS API on a single
loopback port (`19000`); no exposed write surface for browsers or
other tenants.
