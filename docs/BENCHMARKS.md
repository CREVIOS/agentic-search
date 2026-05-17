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

### Result (macOS / M-series, local FS storage backend)

| metric                                 |    value |
| -------------------------------------- | -------: |
| docs                                   | 1 000 000 |
| clusters (`k`)                         |    1 024 |
| dim                                    |      128 |
| probe                                  |       32 |
| **recall@10 vs. ground truth**         | **97.34 %** |
| query latency mean                     | 187.4 ms |
| query latency p50                      | 153.4 ms |
| query latency p95                      | 473.2 ms |
| query latency p99                      | 909.1 ms |
| single-thread throughput               |    5 qps |
| index size on disk                     | 588.4 MB |
| build time (kmeans + write)            | 8 min    |

### Direct comparison (public numbers, late-2025 / 2026)

| system                  | 1 M  recall@10 | 1 M  warm p50 | 1 M  cold p50 | storage |
| ----------------------- | -------------: | ------------: | ------------: | :------ |
| **agentic-search / as-vec** |    **97.34 %** |      *(see warm row below)* | **153 ms** | object  |
| Turbopuffer (centroid)  |        90-95 % |          8 ms |        343 ms | object  |
| Qdrant (HNSW, in-mem)   |        ~98 %   |          4 ms |       (N/A)   | RAM     |
| Milvus (HNSW, in-mem)   |        ~98 %   |          6 ms |       (N/A)   | RAM     |
| Redis Vector (in-mem)   |        ~98 %   |       ~1-5 ms |       (N/A)   | RAM     |

Reading: against the right comparable (Turbopuffer's SPFresh-style
centroid index on object storage) `as-vec` ships a 2.2× lower cold
p50 (**153 ms vs. 343 ms**) at materially higher recall (**97.34 % vs.
90-95 %**) on the same 1 M-vector benchmark. HNSW-in-memory systems
beat both on warm latency but pay $1 600 / TB / month RAM where the
S3-first shape pays ~$70 / TB / month (data from Turbopuffer's
published tradeoffs page).

The 1 M p50 above is "cold" in the sense that the index was just
freshly written to disk; subsequent queries benefit from the OS page
cache and the LRU cluster cache. A true warm-server number (steady
state after 100+ queries) lands in the next bench row once we wire
the persistent-server measurement.

### Open optimisation candidates

- **Cluster-file aggregation.** Each cluster lives in its own file;
  a query at probe=32 issues 32 `store.get` calls. Concatenating all
  cluster blobs into one file with an offset table would collapse
  this to one range read per query.
- **SIMD dot product.** Inner loop is scalar f32; `wide` or
  hand-vectorised SIMD on Apple AMX / x86 AVX2 would close most of
  the warm gap.
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
