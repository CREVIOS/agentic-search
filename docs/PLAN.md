# agentic-search — ULTRAPLAN

## Mission
Fastest agentic search substrate. S3-native filesystem for agents. Hybrid: ripgrep lexical + vector ANN + web search. Rust core. Bindings for Claude Agent SDK, DeepAgents, LangChain, CrewAI. Beat realtime SOTA on latency AND recall.

## Why now
- Agents stall on retrieval. Tool latency dominates loop. p50 search > p50 LLM token.
- ChromaDB / Pinecone / Qdrant assume local FS or hosted cluster. None search S3 directly.
- ripgrep is the fastest lexical scanner. Nobody wired it to agent toolcalls over object storage.
- Agents need *file-like* semantics (open, glob, grep, read range) AND *semantic* (embed, ANN). Two paradigms, one tool.

## Differentiators (must hold all 4)
1. **S3-as-FS**: zero local copy, range-read driven, prefix-glob native. Hot shards cached on NVMe with LRU.
2. **Rust hot path**: zero-copy, mmap where possible, SIMD on tokenize + cosine. `ripgrep` as lib not subprocess. `tantivy` for inverted, `usearch`/`hnsw_rs` for ANN.
3. **Hybrid first-class**: BM25 + dense + reranker fused in one query plan. Not stitched in Python.
4. **SDK-agnostic toolcalls**: one binary, one HTTP+gRPC server, official adapters for Claude Agent SDK, DeepAgents, LangChain, CrewAI, MCP server.

## Architecture (layers)
```
┌────────────────────────────────────────────────────────┐
│  SDK adapters: claude-agent-sdk, deepagents, langchain │
│  + MCP server (stdio + http) + REST + gRPC             │
├────────────────────────────────────────────────────────┤
│  Query planner: BM25/Vec/Web fusion, RRF + reranker    │
├──────────────┬──────────────┬──────────────┬───────────┤
│  Lexical     │  Vector      │  Web         │  Files    │
│  (tantivy +  │  (usearch    │  (brave/     │  (S3 FS,  │
│   ripgrep)   │   HNSW)      │   serpapi)   │   range)  │
├──────────────┴──────────────┴──────────────┴───────────┤
│  Storage: object store (s3, r2, gcs), local NVMe cache │
│  + manifest (parquet) + WAL                            │
└────────────────────────────────────────────────────────┘
```

## Core crates (workspace)
- `as-core` — types, errors, config
- `as-store` — S3 client (`aws-sdk-s3`), GCS, R2, local. Range-read, multipart, async. Cache layer (`foyer` or hand-rolled LRU on NVMe).
- `as-fs` — virtual filesystem over `as-store`. POSIX-ish (open, read_at, list, glob).
- `as-lex` — ripgrep-as-lib (`grep-searcher`, `grep-regex`) + tantivy inverted index over S3-backed segments.
- `as-vec` — embedding (BGE-small via candle / fastembed-rs), HNSW index (`usearch` FFI or `hnsw_rs`), quantization (PQ/SQ).
- `as-web` — pluggable web search (Brave, Tavily, SerpAPI, Exa).
- `as-rerank` — cross-encoder rerank (jina / bge-reranker via candle).
- `as-plan` — query planner, RRF fusion, budget-aware.
- `as-server` — axum HTTP + tonic gRPC + MCP stdio.
- `as-cli` — `agentic-search` CLI: index, query, serve, bench.
- `as-bindings-py` — pyo3 bindings.
- `as-bindings-node` — napi-rs bindings.

## SDK adapters (separate dirs)
- `sdks/python/claude_agent_search/` — wraps as-bindings-py, exposes `tool()` decorated functions for Claude Agent SDK
- `sdks/python/deepagents_search/` — DeepAgents-compatible tool spec
- `sdks/python/langchain_agentic_search/` — Retriever + Tool
- `sdks/node/@agentic-search/sdk/` — TS bindings
- `mcp/` — MCP server (stdio + http) so any MCP client gets it for free

## Performance targets (must beat)
| Metric | Target | Baseline (Chroma local) |
|---|---|---|
| p50 hybrid query @ 10M docs | < 25ms | ~120ms |
| p99 hybrid query @ 10M docs | < 80ms | ~600ms |
| Cold S3 grep, 1GB prefix | < 800ms | n/a (no one does this) |
| Warm S3 grep, 1GB prefix | < 60ms | n/a |
| Index 1M docs (768-d) | < 90s | ~280s |
| RAM @ 10M docs | < 4GB | ~12GB |
| Recall@10 vs BM25-only | +18pp | — |
| Recall@10 vs dense-only | +6pp | — |

## Accuracy plan
- Hybrid retrieval: BM25 (tantivy) ∪ dense (HNSW) → RRF (k=60) → cross-encoder rerank top-50 → return top-k.
- Realtime updates: WAL append → in-memory delta index → background merge to S3.
- Eval: BEIR subset (MS MARCO, NQ, HotpotQA, FiQA, SciFact), LongBench-Retrieval, custom agent-trace benchmark.

## Benchmarks
- `bench/` workspace crate using `criterion` for micro.
- `bench/macro/` python harness: ingests BEIR, runs Chroma / Qdrant / Pinecone / agentic-search, dumps CSV + plots.
- GitHub Actions matrix: nightly runs on `c7gd.4xlarge` and `m7i.2xlarge` (or just local M-series for v0).
- Output: `bench/results/YYYY-MM-DD.json` + `benchmarks.md` table auto-updated.

## Milestones
- **M0 (today)**: repo skeleton, workspace, CI, README, license, plan doc, first commit batch. Stub crates with traits.
- **M1**: `as-store` S3 + local impl. `as-fs` glob/read. CLI `agentic-search ls s3://...` and `grep`. Integration test against MinIO.
- **M2**: `as-lex` tantivy + ripgrep wiring. Index command. Query command. BM25 working.
- **M3**: `as-vec` fastembed + usearch. Hybrid + RRF in `as-plan`.
- **M4**: `as-web` Brave adapter. `as-rerank` bge-reranker via candle.
- **M5**: `as-server` axum + MCP stdio. `as-bindings-py`. Claude Agent SDK adapter.
- **M6**: bench harness vs Chroma/Qdrant. Plots. README numbers.
- **M7**: DeepAgents, LangChain, CrewAI adapters. Node bindings.
- **M8**: realtime WAL + delta index. PQ quantization. SIMD cosine.

## Risks / mitigations
- S3 RTT dominates cold. Mitigation: aggressive prefetch, manifest co-location, NVMe LRU, range-read coalescing.
- Embedding latency. Mitigation: fastembed-rs (ONNX runtime), batch GPU optional, pre-embed at ingest.
- Reranker latency. Mitigation: skip for low-budget queries, run top-50 only, INT8.
- Rust + Python + Node release matrix. Mitigation: maturin for py, napi for node, CI release wheels.

## Repo layout
```
/
├── Cargo.toml (workspace)
├── crates/
│   ├── as-core/
│   ├── as-store/
│   ├── as-fs/
│   ├── as-lex/
│   ├── as-vec/
│   ├── as-web/
│   ├── as-rerank/
│   ├── as-plan/
│   ├── as-server/
│   ├── as-cli/
│   ├── as-bindings-py/
│   └── as-bindings-node/
├── sdks/
│   ├── python/
│   └── node/
├── mcp/
├── bench/
├── docs/
│   ├── PLAN.md (this)
│   ├── ARCHITECTURE.md
│   └── BENCHMARKS.md
├── .github/workflows/
├── examples/
├── README.md
└── LICENSE (Apache-2.0)
```

## Non-goals
- Not a vector DB SaaS. Library + self-host server.
- Not a general embedding model trainer.
- Not a web crawler.

## Open questions (resolve as we go)
- usearch (Apache-2.0 binding via FFI) vs `hnsw_rs` (pure Rust). Pick usearch for raw perf, fallback hnsw_rs.
- Embedding default: BGE-small-en-v1.5 (384d, fast) vs Snowflake arctic-embed-s. Bench both.
- Reranker default: bge-reranker-v2-m3 (multilingual, big) vs jina-reranker-tiny. Default tiny, opt-in big.
- Auth model for S3: env / IAM / profile / explicit creds in config.
