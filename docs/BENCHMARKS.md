# Benchmarks

## Goals (must clear at v1)

| Metric                                         | Target  | Notes                                  |
| ---------------------------------------------- | ------- | -------------------------------------- |
| p50 hybrid query @ 10M docs                    | < 25ms  | warm cache, 384-d embed                |
| p99 hybrid query @ 10M docs                    | < 80ms  | tail-tolerant fusion                   |
| Cold S3 grep, 1GB prefix                       | < 800ms | range-coalesced, prefetched manifest   |
| Warm S3 grep, 1GB prefix                       | < 60ms  | NVMe LRU hits                          |
| Index 1M docs (768-d)                          | < 90s   | batched embedding, parallel HNSW build |
| Resident RAM @ 10M docs                        | < 4GB   | PQ-compressed vectors                  |
| Recall@10, hybrid vs BM25-only (BEIR avg)      | +18pp   | RRF + cross-encoder rerank             |
| Recall@10, hybrid vs dense-only (BEIR avg)     | +6pp    | adds lexical signal                    |

## Workloads

- **BEIR (subset)** — MS MARCO, NQ, HotpotQA, FiQA, SciFact, TREC-COVID.
- **LongBench-Retrieval** — long-context recall, needle-in-haystack.
- **Agent-trace** — replays real Claude Agent SDK tool-call traces; measures
  end-to-end loop latency, not just the retrieval RPC.

## Engines compared

- agentic-search (this project)
- ChromaDB (local persistent)
- Qdrant (local docker)
- Pinecone serverless (network bound; reported separately)

## Harnesses

- **Micro** — `cargo bench -p bench`. Criterion. RRF, tokenize, cosine, HNSW
  query, tantivy query.
- **Macro** — `bench/macro/run.py`. Spins each engine, ingests a workload,
  records latency + recall, emits `bench/results/YYYY-MM-DD.json` and Markdown.

## Reproducibility

- CI nightly runs the macro harness on a fixed runner.
- Results are committed under `bench/results/` with the runner SKU recorded.
- A `benchmarks.md` summary table is regenerated on each run.

_The numbers above are the targets to beat. Once M6 lands we will publish
measured numbers here next to the targets._
