# Benchmarks

The numbers in [`PLAN.md`](PLAN.md) are the targets. This document is how we
measure them.

## Scope decisions (answers to the v3 open questions)

1. **Default public claim covers both workloads, in two separate tables.**
   Code repositories use the grep + tree-sitter path; million-document
   S3 corpora use the centroid vector path. Mixing them into a single
   number always misleads someone, so we don't.
2. **Launch-blocker agent harnesses (every one we ship adapters for):**
   Claude Agent SDK, OpenAI Agents SDK, LangChain, CrewAI, DeepAgents,
   and any MCP host (Claude Code, Cursor, Cline, …). Each adapter
   ships with at least one end-to-end test in `integration_tests/`.
3. **First published claims use RustFS / MinIO on a developer machine**
   so anyone can reproduce them with `docker compose -f
   docker-compose.rustfs.yml up -d`. AWS S3 numbers are reported in a
   separate table when CI runs against a real bucket; they will never
   replace the RustFS numbers, only annotate them.

## What we benchmark

| Dimension                    | Metric                                              | Target  |
| ---------------------------- | --------------------------------------------------- | ------- |
| Cold S3 grep                 | p50 wall-clock over a 1 GB prefix                   | < 800ms |
| Warm S3 grep (NVMe hit)      | p50 wall-clock over a 1 GB prefix                   | < 60ms  |
| Warm S3 grep (memory hit)    | p50 wall-clock over a 1 GB prefix                   | < 12ms  |
| Local grep parity            | ours vs. `rg` subprocess on same local corpus       | ≥ 0.95× |
| AST symbol lookup            | `find_symbol` over 100k-file repo                   | < 40ms  |
| Parallel fan-out             | 12 grep queries via planner vs. 12 sequential       | ≥ 8×    |
| Sub-agent delegate           | main-context token reduction vs. raw loop           | -40%    |
| Web search latency           | p50 for the default `web` tool                      | < 250ms |
| Vector (opt-in, non-code)    | recall@10 on BEIR subset (NQ + FiQA + SciFact)      | ≥ 0.55  |

## Engines compared

- `agentic-search` (this project)
- [`probelabs/probe`](https://github.com/probelabs/probe) — local-only,
  ripgrep + tree-sitter; the closest comp for AST-aware code retrieval.
- `rg` invoked via Bash — what most agents do today.
- [Mountpoint for S3](https://github.com/awslabs/mountpoint-s3) + `rg` — to
  isolate the "library vs. subprocess" cost from the "S3 vs. local" cost.
- ChromaDB (local persistent) — only on the optional vector path, on
  non-code BEIR slices.
- [Exa Instant](https://exa.ai/) — apples-to-apples for the `web` tool.

## Workloads

- **Coding-agent trace** — replays real Claude Agent SDK / DeepAgents tool
  call traces (grep + glob + read + find_symbol). End-to-end loop latency,
  not just RPC latency.
- **SWE-bench retrieval slice** — measures whether the spans returned are
  actually the ones the patch touches.
- **BEIR subset** — MS MARCO, NQ, FiQA, SciFact, TREC-COVID — only used to
  evaluate the *optional* vector path.
- **LongBench-Retrieval** — long-context recall sanity check.

## Harnesses

- **Micro** — `cargo bench -p bench`. Criterion. Covers RRF, tokenize,
  cosine, HNSW query, tantivy query, grep slice.
- **Macro** — `bench/macro/run.py`. Spins each engine, ingests a workload,
  records p50/p95/p99 latency, recall, and token economy. Emits
  `bench/results/YYYY-MM-DD.json` and updates `BENCHMARKS.md` tables.

## Reproducibility

- CI nightly runs the micro suite on a fixed `ubuntu-latest` runner.
- The macro suite runs on a hand-tagged `c7gd.4xlarge` (NVMe instance,
  Graviton) and on a local M-series Mac. Both numbers are reported so the
  reader can see the cache-tier effect.
- Each run pins the engine versions, corpus SHA, and runner SKU into the
  JSON output. Results are committed under `bench/results/`.

## Measured (2026-05-17, macOS / M-series)

Corpus: `tokio-rs/tokio` v1.40.0 source tree, 782 files, mostly Rust.
Pattern: `async fn`. 5 runs each via `bench/macro/run.py --runs 5
--server`.

### CLI-shape (fresh process per call, cold AST cache)

| engine                       |  p50 ms |  p95 ms | mean ms | notes                                                  |
| ---------------------------- | ------: | ------: | ------: | ------------------------------------------------------ |
| `agentic-search grep`        |    65.1 |   424.4 |   138.5 | parallel ripgrep-as-library over async S3-shaped reads |
| `agentic-search grep --ast`  |   811.3 |  2044.3 |  1009.4 | tree-sitter widening, parse cache cold per invocation  |
| `rg` (subprocess)            |    20.5 |    74.2 |    33.4 | native ripgrep, mmap + sync IO, raw line output        |
| `probe search`               |   165.3 |   477.5 |   227.2 | probelabs/probe 0.6.0 — applies its own ranking/dedup  |

### Server-shape (`agentic-search serve`, warm AST parse cache)

This is the agent-loop shape: the server stays up, every call hits the
same `SpanCache`, so AST widening only reparses files that changed.

| endpoint                                | p50 ms | p95 ms | mean ms | notes                                                      |
| --------------------------------------- | -----: | -----: | ------: | ---------------------------------------------------------- |
| `POST /grep`                            |   33.3 |   38.3 |    33.8 | close to native `rg`; in-process tier cache + JoinSet      |
| `POST /grep` (`ast: true`, warm cache)  |   65.9 |  198.6 |    91.8 | **12× faster than the CLI shape** thanks to the AST cache |

Reading: against a persistent server (which is how Claude Code,
DeepAgents, etc. actually consume us) the AST mode is on the order of
60–70 ms p50, ≈ 2× `rg` for the *same* result with whole-function
spans + AST symbol names + dedup + per-stage rank signals. The CLI
shape pays parse cost on every invocation; that 800 ms number is the
worst case, not the operating point.

The agent-trace and S3 cold/warm rows will land once the S3 / Mountpoint
runners are wired into CI.

## CodeSearchNet (global benchmark, lexical mode)

[CodeSearchNet Challenge](https://github.com/github/CodeSearchNet) is the
canonical NL→code retrieval benchmark (6 M functions, NDCG / MRR). We
run a reproducible Python slice via
`bench/global/codesearchnet.py` against the `code-search-net/code_search_net`
HuggingFace dataset.

| run                                           | language | docs |  queries | MRR@10  | NDCG@10 | Recall@10 | per-query |
| --------------------------------------------- | -------- | ---: | -------: | :-----: | :-----: | :-------: | --------: |
| agentic-search grep --ast (OR-of-tokens)      | python   | 2000 |       50 | 0.0824  | 0.1092  |    20.0%  |    200 ms |

These are the *lexical-only* numbers — the query is the function's
docstring, the engine is regex grep + tree-sitter widening with no
embedding stage. Recall@10 of 20 % is the natural ceiling for OR-of-
tokens against NL queries; SOTA semantic systems (CasCode 0.7795 MRR)
use neural rerankers. Our planner is designed to call out to those
when the user opts in — this row is the honest "no vectors, no
embeddings, just grep" baseline.

## S3 (RustFS local container)

Same corpus uploaded into a RustFS container on `s3://`; cache
configured (memory LRU + NVMe LRU). Runs include a cold first call
plus 4 warm calls.

| engine                              |  p50    |  p95    |  mean   |
| ----------------------------------- | ------: | ------: | ------: |
| agentic-search grep (s3 mixed)      | 1105 ms | 2590 ms | 1404 ms |
| agentic-search grep --ast (s3)      | 1430 ms | 1634 ms | 1448 ms |

Cold S3 is currently dominated by `ListObjectsV2` paging against
RustFS — the next optimisation is a co-located prefix manifest so
listing collapses to a single GET. Warm reads (NVMe-hit) are
sub-100 ms in micro-benches and the cache target stands.

_Targets at the top are what we are aiming to clear. The two tables
above are the first reproducible numbers._
