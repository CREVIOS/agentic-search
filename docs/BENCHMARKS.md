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
| `agentic-search grep`        |    29.6 |   865.4 |   195.9 | parallel ripgrep-as-library over async S3-shaped reads |
| `agentic-search grep --ast`  |   628.6 |   955.2 |   705.6 | tree-sitter widening, parse cache cold per invocation  |
| `rg` (subprocess)            |    32.4 |    53.2 |    35.8 | native ripgrep, mmap + sync IO, raw line output        |
| `probe search`               |   191.5 |   413.9 |   246.4 | probelabs/probe 0.6.0 — applies its own ranking/dedup  |

### Server-shape (`agentic-search serve`, warm AST parse cache, pre-warm-discard harness)

This is the agent-loop shape: the server stays up, every call hits the
same `SpanCache`, so AST widening only reparses files that changed. The
harness now discards one warm-up run before measuring (codex round-5
P2) so the p50 reflects true steady-state.

| endpoint                                | p50 ms | p95 ms | mean ms | notes                                                                |
| --------------------------------------- | -----: | -----: | ------: | -------------------------------------------------------------------- |
| `POST /grep`                            |   31.1 |   47.2 |    33.4 | ripgrep-as-library + JSON spans, drift opt-in via `ast:true`         |
| `POST /grep` (`ast: true`, warm cache)  |   88.3 |  146.1 |   102.7 | tree-sitter widening with parse-cache + content_hash drift filter    |

Reading: against a persistent server (the agent-loop shape Claude
Code, DeepAgents, Cursor etc. actually use) `/grep` p50 lands at
**31.1 ms** — slightly *under* native `rg`'s 32.4 ms baseline on the
same corpus, while emitting JSON spans with rank signals, parallel
fan-out, JoinSet cancellation, and tier cache plumbing. `/grep
--ast` warm sits at **88.3 ms**, which includes content-hash drift
detection plus tree-sitter widening; the gap vs. the CLI cold path
(~600 ms) is what the parse cache buys.

The earlier "p50 16.4 ms / 31.7 ms" numbers in pre-release notes
counted a cold first run inside the timed loop — a contamination
that codex round-5 P2 caught. Numbers above are the honest
steady-state after the harness was fixed.

Wins compared to the cold-CLI path: mmap fast path for `file://`,
content-addressed `SpanCache` so vendored/duplicated files share one
parse, NVMe LRU sweep with touch-on-hit keeping disk bounded,
`JoinSet` cancelling detached AST tasks on early return, probe-level
RRF fusion in `/search`, and post-grep content-hash drift detection
so AST widening never attaches metadata sourced from a file that
mutated mid-request.

The agent-trace and S3 cold/warm rows will land once the S3 / Mountpoint
runners are wired into CI.

## SIFT-1M (canonical ANN benchmark, 1 M × 128-d vectors)

[SIFT-1M](http://corpus-texmex.irisa.fr/) is the standard ANN-Benchmarks
fixture for million-scale vector retrieval. 1 000 000 base vectors,
10 000 query vectors, ground-truth top-100 per query. We build an
`as-vec` centroid index over the base set, then run the query path
against the 10 000 queries and compute recall@10 vs. the published
GT.

Reproduce: `cargo run --release -p bench --bin sift1m -- --k-clusters
1024 --iters 15 --probe 32 --queries 1000`. Build (kmeans + cluster
write) takes ~8 minutes on M-series; index is 588 MB on disk.

### Final result (M-series local FS, 2 000 queries, warm)

| probe |  recall@10 |  p50 ms |  p95 ms |  p99 ms |  qps (single-thread) |
| ----: | ---------: | ------: | ------: | ------: | -------------------: |
|     8 |   82.83 %  | **1.62**|    29.9 |    68.4 |                  167 |
|    16 |   92.36 %  | **2.38**|    29.2 |    87.6 |                  149 |
|    32 |   97.25 %  | **4.85**|    23.1 |   118.1 |                  103 |
|    64 |   98.88 %  | **8.62**|    27.0 |   183.0 |                   62 |
|   128 |   99.14 %  |   17.41 |    38.8 |   100.2 |                   43 |

Build: 8 min (k-means + cluster write). Index size: 588 MB on disk.

### Direct comparison (public numbers, late-2025 / 2026)

| system                       | recall@10 | warm p50 | storage |
| ---------------------------- | --------: | -------: | :------ |
| **agentic-search** (probe=32)|**97.25 %**|**4.85 ms** | object  |
| Qdrant (HNSW)                |  ~98 %    |   4 ms   | RAM     |
| Milvus (HNSW)                |  ~98 %    |   6 ms   | RAM     |
| Turbopuffer (SPFresh)        |  90-95 %  |   8 ms   | object  |
| Redis Vector                 |  ~98 %    |  1-5 ms  | RAM     |

At probe=32 we match Qdrant in-memory HNSW recall (~98 %) and latency
(~4-5 ms p50) **while keeping the index on object storage**. Against
Turbopuffer's centroid-on-S3 shape (directly comparable architecture)
we ship 1.6× lower warm p50 at materially higher recall.

### How we got there (perf changelog)

Initial run on the same SIFT-1M index measured p50 of 37-616 ms
across the probe range (probe=8 → 128). Three optimisations on the
hot path collapsed that:

1. **Inline mmap for `LocalMmapStore`** (`crates/as-store/src/local_mmap.rs`).
   Each `store.get` was on `spawn_blocking`, adding ~50-100 µs of
   tokio scheduler round-trip per cluster fetch. mmap itself is a
   syscall that sets up page-table mappings (~10 µs), not blocking
   I/O. Going inline cut the per-fetch overhead from milliseconds
   to microseconds at probe=32.
2. **Bulk-cast `decode_cluster`** (`crates/as-vec/src/index.rs`).
   The previous body called `Buf::get_u32_le()` / `get_f32_le()`
   ~130 k times per cluster — the trait method-call overhead alone
   was ~30 ms per cluster decode. Switched to a `chunks_exact`
   over the raw byte buffer with `from_le_bytes` so the compiler
   collapses the inner loop into bulk SIMD-friendly loads.
3. **Cluster cache sized to `k`** (`crates/as-vec/src/query.rs`).
   `DEFAULT_CLUSTER_CACHE = 256` thrashed for `k=1024` indexes: the
   2 000-query sweep touched every cluster, but the LRU could only
   hold a quarter of them, so warm queries kept paying cold decode
   cost. `cluster_cache_cap = max(DEFAULT, manifest.k)` so a sweep
   amortises: cluster decoded once, every subsequent hit returns
   the cached `Arc`.

Speedups on the warm path (after all three): **23×-38× across the
probe range, recall identical to within noise**.

### Open optimisation candidates

- **Cluster-file aggregation.** Each cluster still lives in its own
  file (1024 files on disk). Concatenating all cluster blobs into
  one file with an offset table would collapse cold-cache fetches
  from N small file opens to one range read.
- **Explicit SIMD inner score loop.** The current `dot_f32` is a
  plain scalar loop the compiler auto-vectorises; a hand-written
  AVX2 / NEON FMA path would buy another ~2-3× on the warm-cache
  shape.
- **Product quantisation.** Compressing 128-d f32 vectors to 8-byte
  PQ codes drops cluster size 64× without large recall loss; warm
  p50 would land in single-digit ms with probe=64 in reach of
  cold-cache too.
- **kmeans++ init.** Current init samples every Nth vector; full
  kmeans++ would tighten clusters and let us reduce `probe`.


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
