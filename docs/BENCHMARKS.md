# Benchmarks

The numbers in [`PLAN.md`](PLAN.md) are the targets. This document is how we
measure them.

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

_The targets above are what we are aiming to clear. The first row of real
measurements will land here with M2 and M3._
