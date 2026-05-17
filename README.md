# agentic-search

> S3-native search runtime for AI agents. Ripgrep linked as a library,
> tree-sitter spans, Turbopuffer-style centroid vector index — all behind
> one MCP server and one REST endpoint that every agent runtime can call.

[![CI](https://github.com/CREVIOS/agentic-search/actions/workflows/ci.yml/badge.svg)](https://github.com/CREVIOS/agentic-search/actions/workflows/ci.yml)
[![Security](https://github.com/CREVIOS/agentic-search/actions/workflows/security.yml/badge.svg)](https://github.com/CREVIOS/agentic-search/actions/workflows/security.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.78%2B-orange.svg)](https://www.rust-lang.org/)

```text
agent → MCP / REST → planner → ┬─ grep   (ripgrep-as-lib)         ─┐
                                ├─ AST    (tree-sitter spans)        │  RRF fuse
                                └─ vector (centroid index, optional) ─┘
                                                ↑
                                          tier cache: mem → NVMe → S3
```

`agentic-search` runs your agent's `ls / glob / read / grep / find_symbol`
calls directly against object storage (S3, R2, GCS, Mountpoint, S3 Files,
or a local path) without copying or sync-ing the corpus first. The hot
path is ripgrep linked in-process — no `Popen("rg")`. AST widening uses
tree-sitter so matches expand to whole functions/classes, not half-cut
chunks. The optional vector path is a Turbopuffer-shaped centroid index
that lives on the same object store.

## Why

By 2026 every major coding agent — Claude Code, Cursor, Codex CLI,
Devin, Aider, OpenCode, Continue — has converged on the same retrieval
loop:

```text
agent → parallel(grep, glob, read) → tree-sitter spans → reason → repeat
```

The bottleneck is the **latency and ergonomics of the file tools**,
especially when the agent's "filesystem" is an S3 bucket. See
[`docs/RESEARCH.md`](docs/RESEARCH.md) for the receipts.

## Highlights

- **One binary, every agent.** MCP stdio + REST + Python + Node + Go SDKs.
- **S3 is the filesystem.** Raw S3, R2, GCS, Mountpoint-S3, S3 Files (NFS),
  or `file://` — same API.
- **Ripgrep linked, not exec'd.** `grep-searcher` runs in-process; no
  shell escaping, no process startup tax.
- **Tree-sitter spans.** Matches widen to enclosing `fn`/`class`/`method`
  via a parse-once-per-file `ContainerIndex` + content-addressed cache.
- **Tier cache** (Turbopuffer-style): in-memory LRU → NVMe LRU with mtime
  sweep → object store. Warm queries approach local-FS speed.
- **Parallel fan-out + RRF fusion.** One `/search` call issues multiple
  grep + AST probes on a `JoinSet` and fuses results with reciprocal-rank
  fusion. Cancel-on-drop, no leaked tasks. The centroid vector index can
  be fused in alongside when the corpus is indexed and the path is
  enabled.
- **Centroid vector index** (Turbopuffer-shaped). 2-roundtrip query on
  object storage. Off by default; enable when the corpus is non-code or
  semantic recall matters.
- **Cold-S3 manifest.** Optional prefix manifest collapses `ListObjectsV2`
  paging into one GET for million-document buckets.
- **Production-grade security defaults.** Server binds `127.0.0.1` unless
  `--allow-public`; path-escape rejection; `gitleaks` + `cargo-deny` in CI.

## Install

### CLI (Rust)

```bash
# install the `agentic-search` binary from the as-cli crate
cargo install --git https://github.com/CREVIOS/agentic-search --locked as-cli
agentic-search --version
```

Or build from source:

```bash
git clone https://github.com/CREVIOS/agentic-search
cd agentic-search
cargo install --path crates/as-cli --locked
```

Pre-built binaries for linux/darwin/windows (x86_64 + arm64) are attached
to every [GitHub Release](https://github.com/CREVIOS/agentic-search/releases).

### Docker

```bash
# pull the multi-arch image from GHCR (each release pushes a fresh tag)
docker run --rm -p 127.0.0.1:8787:8787 ghcr.io/crevios/agentic-search:latest

# or build + run with the included compose file (persists the cache)
docker compose up -d
curl -s http://127.0.0.1:8787/health
```

The image runs as a non-root user, persists the NVMe LRU + fastembed
model cache to a named volume, and binds `127.0.0.1:8787` on the host
by default. Multi-arch (linux/amd64 + linux/arm64) images are pushed
to `ghcr.io/crevios/agentic-search:<version>` on every tagged release.
Override `AWS_*` env vars in a `.env` file alongside `docker-compose.yml`
to point the server at an S3 / R2 / GCS bucket.

### Python

Framework-specific adapters (each one spawns / talks to the CLI):

```bash
pip install claude-agent-search        # Claude Agent SDK
pip install openai-agentic-search      # OpenAI Agents SDK
pip install deepagents-search          # DeepAgents
pip install langchain-agentic-search   # LangChain Retriever + Tool
pip install crewai-agentic-search      # CrewAI tool wrapper
```

### Node / TypeScript

```bash
pnpm add @agentic-search/sdk
# or: npm i @agentic-search/sdk
```

### Go

```bash
go get github.com/CREVIOS/agentic-search/sdks/go/agenticsearch
```

## Quickstart

### CLI verbs

```bash
# treat an S3 prefix as a working directory
agentic-search ls    s3://my-corpus/docs/
agentic-search glob  s3://my-corpus/docs/ "**/*.md"
agentic-search grep  s3://my-corpus/      "TODO\\(security\\)"
agentic-search find  s3://my-repo/src/    --symbol verify_jwt

# build a prefix manifest so cold listing collapses to one GET
agentic-search index-manifest s3://my-corpus/

# run as MCP stdio server (any MCP host can attach)
agentic-search serve --mcp

# run as REST server (default 127.0.0.1:8787)
agentic-search serve
```

### MCP (Claude Code, Cursor, Cline, …)

```json
{
  "mcpServers": {
    "agentic-search": {
      "command": "agentic-search",
      "args": ["serve", "--mcp"]
    }
  }
}
```

### Claude Agent SDK

```python
from claude_agent_sdk import ClaudeAgentOptions, query
from claude_agent_search import mcp_server_config, as_tools

opts = ClaudeAgentOptions(
    mcp_servers=mcp_server_config(),
    allowed_tools=as_tools(),
)

async for msg in query(
    prompt="Find every place we still use HS256 in s3://corp/ and summarize.",
    options=opts,
):
    print(msg)
```

### DeepAgents

```python
from deepagents import create_deep_agent
from deepagents_search import search_tool

agent = create_deep_agent(tools=[search_tool()])
```

### Node / TypeScript

```ts
import { AgenticSearchClient } from "@agentic-search/sdk";

const client = new AgenticSearchClient("http://127.0.0.1:8787");
const hits = await client.grep("s3://corp/", "HS256");
console.log(hits);
```

### Go

```go
import "github.com/CREVIOS/agentic-search/sdks/go/agenticsearch"

c := agenticsearch.New("http://127.0.0.1:8787")
hits, _ := c.Grep(ctx, "s3://corp/", "HS256", nil)
```

## Benchmarks

Numbers below are from a 782-file Rust corpus (`tokio-rs/tokio` v1.40),
5 runs, macOS / M-series. Full methodology and additional tables in
[`docs/BENCHMARKS.md`](docs/BENCHMARKS.md).

### Server-shape (agent-loop, warm AST cache, pre-warm-discard harness)

| endpoint                                | p50 ms | p95 ms | mean ms | notes                                              |
| --------------------------------------- | -----: | -----: | ------: | -------------------------------------------------- |
| `POST /grep`                            |   31.1 |   47.2 |    33.4 | ripgrep-as-library + JSON spans                    |
| `POST /grep` (`ast: true`, warm cache)  |   88.3 |  146.1 |   102.7 | tree-sitter widening + parse-cache + drift check    |
| `rg` (subprocess)                       |   32.4 |   53.2 |    35.8 | native ripgrep baseline, raw line output           |

Reading: against a persistent server (the agent-loop shape Claude Code,
DeepAgents, Cursor etc. actually use) `/grep` lands at p50 **31.1 ms** —
slightly **under** native `rg`'s 32.4 ms baseline on the same corpus,
while emitting JSON spans, parallel fan-out, JoinSet cancellation, and
tier-cache plumbing. `ast: true` warm sits at p50 **88.3 ms**, which
includes content-hash drift detection plus tree-sitter widening.

The earlier "p50 16.4 ms" claim in pre-release notes was contaminated by
counting a cold first run inside the timed loop. The harness now discards
one warm-up run before measuring (codex P2 fix). These numbers are the
honest steady-state.

Wins compared to the cold-CLI path: mmap fast path for `file://`,
content-addressed `SpanCache` so vendored files share one parse, NVMe LRU
sweep keeps disk bounded, `JoinSet` cancels detached AST work on early
return, probe-level RRF fusion in `/search`, and post-grep content-hash
drift detection so AST widening never attaches metadata sourced from a
file that mutated mid-request.

### CodeSearchNet (lexical-only, no embeddings)

| run                                      | language | docs | queries | MRR@10 | NDCG@10 | Recall@10 |
| ---------------------------------------- | -------- | ---: | ------: | :----: | :-----: | :-------: |
| `agentic-search grep --ast` (OR-tokens)  | python   | 2000 |      50 | 0.0824 | 0.1092  |   20.0%   |

The honest "no vectors, no embeddings, just grep" baseline. Enable the
centroid vector path for semantic recall.

## Architecture

```text
┌──────────────────────────────────────────────────────────────────┐
│  SDK adapters  ·  Claude  DeepAgents  LangChain  CrewAI  OpenAI  │
├──────────────────────────────────────────────────────────────────┤
│  Tool surface  ·  ls  glob  read  grep  find_symbol  search      │
│                ·  delegate (sub-agent isolation)                 │
├──────────────────────────────────────────────────────────────────┤
│  Planner       ·  parallel JoinSet  ·  RRF fusion  ·  dedup      │
├──────────────────┬───────────────────┬───────────────────────────┤
│  Grep            │  AST              │  Vector                   │
│  (rg-as-lib)     │  (tree-sitter)    │  (centroid, opt-in)       │
├──────────────────┴───────────────────┴───────────────────────────┤
│  Tier cache   ·  memory LRU  →  NVMe LRU (mtime sweep)  →  store │
├──────────────────────────────────────────────────────────────────┤
│  Object store ·  s3 · r2 · gcs · s3-files · mountpoint · file    │
└──────────────────────────────────────────────────────────────────┘
```

Crate-level breakdown in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

| Crate         | Purpose                                                   |
| ------------- | --------------------------------------------------------- |
| `as-core`     | Shared error / result types                               |
| `as-store`    | Object-store trait + S3/GCS/R2/local backends, manifest   |
| `as-fs`       | `ls / glob / read` surface                                |
| `as-grep`     | Ripgrep-as-library, parallel scan, span emission          |
| `as-ast`      | Tree-sitter spans, `ContainerIndex`, content-addr cache   |
| `as-cache`    | Tiered cache: memory LRU → NVMe LRU with mtime sweep      |
| `as-embed`    | fastembed-rs (ONNX, BGE-small-en)                         |
| `as-vec`      | Centroid (clustered) vector index on object storage       |
| `as-plan`     | Planner: parallel fan-out, stage budgets, RRF fusion      |
| `as-server`   | REST + MCP stdio server                                   |
| `as-cli`      | `agentic-search` binary                                   |

## Security

- Server binds `127.0.0.1` by default; `--allow-public` required for
  non-loopback bind.
- Path-escape rejection on every `file://` read.
- API keys read from environment only, never logged.
- CI: `cargo-deny` (advisories/bans/licenses/sources) + `gitleaks`.
- MCP transport follows JSON-RPC 2.0 (protocol 2025-11-25).

Report security issues privately via GitHub Security Advisories.

## Status

`v0.1.0` — first public release. Core search path (grep + AST + cache +
RRF fusion + MCP/REST surface) is production-ready; the cold-S3 manifest
and centroid vector index are wired but still tuning their cost/recall
defaults. Server-shape `/grep` already beats native `rg` on the same
corpus while emitting JSON spans. See [CHANGELOG.md](CHANGELOG.md).

## Contributing

```bash
# build + test
cargo test --workspace --locked

# run the benchmark harness
python bench/macro/run.py --runs 5 --server

# local S3 (RustFS) for cold/warm S3 testing
./scripts/rustfs-up.sh
```

CI gates: clippy `-D warnings`, criterion regression, `cargo-deny`,
`gitleaks`, `pip-audit` (5 Python adapters), `pnpm audit` (Node),
`govulncheck` (Go). PRs that change a hot path must include a bench
delta. Full contributor guide in [CONTRIBUTING.md](CONTRIBUTING.md).

## License

Apache-2.0. See [LICENSE](LICENSE).
