# agentic-search

> Fastest agentic search substrate. S3-native filesystem for agents. Hybrid lexical + vector + web. Rust core, Python/Node bindings, MCP + REST + gRPC.

[![CI](https://github.com/claudemakebell/agentic-search/actions/workflows/ci.yml/badge.svg)](https://github.com/claudemakebell/agentic-search/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

## What

`agentic-search` is a search runtime built for AI agents. It treats S3 (or any
object store) as the agent's filesystem and gives the agent **one** tool surface
that spans:

- **`ls`/`glob`/`read_at`** — POSIX-ish access to S3/GCS/R2 prefixes, no local sync
- **`grep`** — ripgrep-as-library scans over object-store ranges (no subprocess)
- **`lexical` (BM25)** — tantivy inverted index, segments live in the store
- **`vector` (ANN)** — fastembed + HNSW, optional product quantization
- **`web`** — Brave / Tavily / SerpAPI / Exa adapters
- **`hybrid`** — RRF fusion of any subset, plus cross-encoder rerank

All wired into a single planner with a budget-aware execution model.

## Why

Modern agent loops are bottlenecked by retrieval. Existing vector DBs
(Chroma/Pinecone/Qdrant) assume a local FS or a hosted cluster and bolt on
lexical as an afterthought. None of them let an agent point at `s3://corpus/`
and search **right there** with hybrid retrieval and the speed of native
ripgrep.

`agentic-search` is opinionated about three things:

1. **S3 is the filesystem.** Agents never have to "sync." Range reads, prefix
   manifests, NVMe LRU cache.
2. **Rust on the hot path.** Tokenization, BM25, ANN, cosine — all native, all
   SIMD where it matters. Ripgrep linked as a library, not exec'd.
3. **One binary, every SDK.** Official adapters for the Claude Agent SDK,
   DeepAgents, LangChain, CrewAI. MCP server out of the box.

## Status

Pre-alpha. Public from day one. See [`docs/PLAN.md`](docs/PLAN.md) for the
roadmap (M0 → M8) and [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md) for the
performance bar we are aiming to clear.

## Quickstart

```bash
# install (once we ship a release binary)
cargo install --path crates/as-cli

# list an S3 prefix as a filesystem
agentic-search ls s3://my-corpus/docs/

# grep across that prefix without downloading anything locally
agentic-search grep s3://my-corpus/docs/ "TODO\\(security\\)"

# build a hybrid (BM25 + vector) index
agentic-search index s3://my-corpus/docs/ --out s3://my-corpus/.index/

# query
agentic-search query s3://my-corpus/.index/ "kerberos token rotation" -k 10

# serve HTTP + MCP for an agent
agentic-search serve --bind 0.0.0.0:8787
```

### From the Claude Agent SDK

```python
from claude_agent_sdk import ClaudeAgentOptions, query
from claude_agent_search import as_tools  # ships in sdks/python/

opts = ClaudeAgentOptions(
    mcp_servers={"agentic_search": {"command": "agentic-search", "args": ["serve", "--mcp"]}},
    allowed_tools=as_tools(),
)
async for msg in query(prompt="Find every place we still use HS256 in s3://corp/", options=opts):
    print(msg)
```

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│  SDK adapters: claude-agent-sdk · deepagents · langchain     │
│  + MCP server (stdio + http) + REST + gRPC                   │
├──────────────────────────────────────────────────────────────┤
│  Query planner: BM25 / Vec / Web fusion (RRF) + reranker     │
├──────────────┬──────────────┬──────────────┬─────────────────┤
│  Lexical     │  Vector      │  Web         │  Files          │
│  tantivy +   │  fastembed   │  Brave /     │  S3 / GCS / R2  │
│  ripgrep     │  + HNSW      │  Tavily      │  range-read FS  │
├──────────────┴──────────────┴──────────────┴─────────────────┤
│  Object store + NVMe LRU cache + manifest (parquet) + WAL    │
└──────────────────────────────────────────────────────────────┘
```

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the layered design and
the per-crate responsibility split.

## Benchmarks

We track latency and recall against Chroma, Qdrant, and Pinecone on BEIR
subsets and a custom agent-trace workload. Results land in
[`docs/BENCHMARKS.md`](docs/BENCHMARKS.md) and `bench/results/`.

## License

Apache-2.0. See [LICENSE](LICENSE).
