# agentic-search — PLAN (v3, Turbopuffer-style for M-doc S3 corpora)

## v3 update (post-pivot)

The agentic loop (grep + tree-sitter + parallel fan-out + cache) is what
agents already get from Claude Code / DeepAgents and is great for source
trees. It does **not** scale to millions of markdown / document objects
in S3, because every query is O(N) reads against the bucket.

For the M-doc S3 case (the user's actual workload — agents currently
copy a full prefix to local FS before searching) we adopt the
[Turbopuffer model](docs/RESEARCH.md):

1. **Object storage is the database**, not a cold tier.
2. **Namespace = S3 prefix.** Each corpus is its own namespace.
3. **Centroid (clustered) ANN, not HNSW.** Query is two roundtrips:
   load centroids, fetch top-`probe` cluster files.
4. **Tier cache** (`as-cache`) progressively inflates hot data: S3 →
   NVMe LRU → memory LRU.
5. **No BM25 / tantivy** on the default path. Vector + grep are the
   only two stages.

Files on object storage:

```
s3://bucket/.agentic-search/index/<ns>/
    manifest.json              version, dim, K, embed_model, sizes
    centroids.f32              K * dim float32 (always in mem)
    cluster_00000.bin          [u32 doc_id, [f32; dim] vec] *
    cluster_00001.bin          ...
    cluster_<K-1>.bin
    docs.jsonl                 per-doc metadata (uri, byte_range, snippet)
```

This is what `as-vec` ships in v3 (was a stub in v2). Build pipeline
chunks → embeds (BGE-small via `as-embed` / fastembed-rs) → trains
k-means → writes segments. Query: embed → top centroids → fetch those
cluster files in parallel → cosine over the union → top-k.

Below is the original v2 plan, retained for reference.

---

# agentic-search — PLAN (v2, post-research)

> Reframed after the [research synthesis](RESEARCH.md). The original plan
> assumed RAG-shaped retrieval was the bottleneck. The evidence says the
> bottleneck for agentic workloads is *file-shaped, S3-native, parallel
> grep with AST-aware spans* — not vector ANN. We now optimize for that.

## Mission

Be the **fastest substrate behind an agent's file tools**.

Agents already converge on `ls / glob / read / grep` (Claude Agent SDK,
DeepAgents, Cursor, Codex CLI, Cline, …). They want those tools to (a) work
directly on S3-class object storage, (b) return AST-aware spans instead of
chunks, (c) fan out in parallel with sub-agent context isolation, (d)
optionally enrich with web search and (e) optionally fall back to vector
retrieval for non-code corpora.

`agentic-search` is the Rust runtime that does all of the above behind one
MCP/REST surface and a thin per-SDK adapter.

## Non-goals (deliberately not what we are)

- We are **not** a general vector DB. Vector retrieval is opt-in, off by
  default, and only really pays off for non-code / unstructured corpora.
- We are **not** a web crawler.
- We are **not** an agent framework. We are a backend for agent frameworks.
- We do **not** ship our own embedding model. We embed externally and store.

## Primary differentiators (must hold all five)

1. **S3 is the filesystem.** Works on raw S3, R2, GCS, Mountpoint, and the
   new S3 Files (NFS). No local copy step.
2. **Ripgrep linked, not exec'd.** `grep-searcher` runs inside the binary;
   no shell escaping, no process startup latency.
3. **Tree-sitter spans, Probe-style.** Matches are expanded to whole
   functions / classes / methods, not raw line ranges.
4. **Tier-cached like Turbopuffer.** Object storage → NVMe LRU → memory. The
   more an agent uses a prefix, the closer it lives to the CPU.
5. **One binary, every SDK.** MCP server + REST + Python + Node bindings,
   with first-party adapters for Claude Agent SDK, DeepAgents, LangChain,
   CrewAI.

## Architecture (new)

```
┌──────────────────────────────────────────────────────────────────┐
│  SDK adapters  ·  Claude Agent SDK   DeepAgents   LangChain      │
│                ·  CrewAI             Node SDK     MCP            │
├──────────────────────────────────────────────────────────────────┤
│  Tool surface  ·  ls  glob  read  grep  search  web  delegate    │
├──────────────────────────────────────────────────────────────────┤
│  Planner       ·  parallel fan-out  ·  dedup by span             │
│                ·  RRF fuse (grep + ast + fname + web)            │
├───────────────┬───────────────┬────────────────┬─────────────────┤
│  Lexical      │  AST          │  Optional      │  Web            │
│  (grep, BM25) │  (tree-sitter)│  vector        │  (Exa/Brave/    │
│               │               │  (fastembed +  │   Tavily)       │
│               │               │   HNSW)        │                 │
├───────────────┴───────────────┴────────────────┴─────────────────┤
│  Cache tier   ·  Memory LRU                                      │
│               ·  NVMe LRU (foyer-style)                          │
│               ·  Manifests (parquet) co-located with data        │
├──────────────────────────────────────────────────────────────────┤
│  Object store ·  s3  ·  r2  ·  gcs  ·  s3-files (NFS)  ·  file   │
└──────────────────────────────────────────────────────────────────┘
```

## Workspace crates (revised)

- `as-core` — types, errors, config.
- `as-store` — object-store abstraction (s3, gcs, r2, file, plus NFS-mounted
  s3-files / mountpoint as a special-cased local path).
- `as-fs` — POSIX-ish virtual filesystem; `ls`, `glob`, `read_at`.
- `as-cache` — **new.** Memory + NVMe tier behind an LRU; cache keys are
  `(store, key, range)` and `(query_hash, prefix)`.
- `as-grep` — **renamed from as-lex.** Ripgrep-as-library (`grep-searcher`,
  `grep-regex`), parallel scan, range-coalescing.
- `as-ast` — **new.** Tree-sitter span extractor (Probe-style). Given a
  match, return the enclosing function/class/method as a span.
- `as-index` — **renamed from as-lex/tantivy.** Optional BM25 index for
  unstructured corpora. Off by default for code.
- `as-vec` — **demoted.** Opt-in (feature flag), off by default. fastembed +
  HNSW for unstructured corpora only.
- `as-web` — Exa default, Brave / Tavily fallback. Returns markdown +
  highlights tuned for tokens.
- `as-plan` — query planner. Parallel fan-out, dedup-by-span, RRF fusion of
  the enabled stages, optional rerank.
- `as-delegate` — **new.** Sub-agent isolation: spawns a search-only loop in
  its own context window and returns a compressed result.
- `as-server` — axum HTTP + MCP stdio + (M5+) gRPC.
- `as-cli` — `agentic-search` CLI.
- `as-bindings-py`, `as-bindings-node` — language bindings.

The original `as-rerank` crate stays around but is no longer on the default
hot path; it is only invoked when the planner sees a non-code corpus or the
user explicitly asks for it.

## Tool surface (what the agent sees)

| Tool          | Description                                                            |
| ------------- | ---------------------------------------------------------------------- |
| `ls`          | List a prefix (`s3://bucket/prefix`).                                  |
| `glob`        | Glob within a prefix; supports double-star.                            |
| `read`        | Read bytes (full file or `[start, end]`).                              |
| `grep`        | ripgrep over a prefix; returns AST-expanded spans.                     |
| `find_symbol` | tree-sitter-only: locate a function/class/method by name.              |
| `search`      | Planner-driven hybrid: grep + ast + fname (+ optional vector + web).   |
| `web`         | Web-only: Exa/Brave/Tavily.                                            |
| `delegate`    | Spawn a search-only subagent loop; return a compressed answer.         |

All tools are streaming and parallel-safe; the planner fans out internally
and returns a single deduplicated result block per call.

## Performance targets (revised)

| Metric                                              | Target  | Notes                              |
| --------------------------------------------------- | ------- | ---------------------------------- |
| Cold S3 grep, 1 GB prefix                           | < 800ms | manifest-prefetched, range-coalesced |
| Warm S3 grep, 1 GB prefix (NVMe-hit)                | < 60ms  | tier cache                         |
| Warm S3 grep, 1 GB prefix (memory-hit)              | < 12ms  | tier cache                         |
| ripgrep-as-lib vs `rg` subprocess on same corpus    | ≥ 0.95× | within 5% of native                |
| `find_symbol` over 100k-file repo                   | < 40ms  | tree-sitter cache                  |
| Parallel fan-out: 12 grep queries vs 12 sequential  | ≥ 8×    | speedup from server-side parallel  |
| `delegate` end-to-end vs raw agent loop             | -40%    | main-context tokens                |

Recall is workload-dependent. Where we ship vector, we will publish numbers
against BEIR subsets; where we ship grep+AST, we will publish numbers
against an internal coding-agent trace suite + SWE-bench retrieval slice.

## Milestones (revised)

- **M0** ✅ Repo skeleton, workspace, CI, license, plan, research doc.
- **M1** — `as-store` (s3 + local) + `as-fs` (ls/glob/read). CLI verbs.
  MinIO integration test.
- **M2** — `as-grep` (ripgrep-as-lib) + `as-ast` (tree-sitter spans) +
  parallel fan-out in `as-plan`. **This is the headline release.**
- **M3** — `as-cache` (memory + NVMe tier). Manifests on object store.
  Realtime invalidation hooks.
- **M4** — `as-web` (Exa default) + `as-delegate` (subagent loop). MCP
  stdio + axum REST in `as-server`.
- **M5** — Python bindings + Claude Agent SDK adapter + MCP config helper.
- **M6** — Benchmarks vs Probe, vs `rg`-via-Bash, vs ChromaDB (where
  vector-on, comparable corpus), vs Exa (web). Numbers published.
- **M7** — DeepAgents + LangChain + CrewAI + Node bindings.
- **M8** — Optional `as-vec` polish (PQ, SIMD cosine, WAL/delta). Only if
  the data shows it pays off for non-code workloads we care about.

## Risks (revised)

- **S3 RTT dominates cold path.** Mitigation: manifest prefetch, NVMe LRU,
  range coalescing — same playbook Turbopuffer uses for vectors.
- **Tree-sitter grammar surface is huge.** Ship the top-10 languages first
  (Rust, TS, JS, Python, Go, Java, C, C++, Ruby, PHP); use plain ripgrep
  spans as fallback for the rest.
- **Probe is well-loved.** Differentiate on S3, MCP, and tier cache; don't
  pretend Probe doesn't exist.
- **MCP / Claude Agent SDK churn.** Pin the adapter packages to specific
  protocol versions; bump on a release schedule, not nightly.

## Open questions

- Default tree-sitter span granularity: function-level vs class-level vs
  configurable? (Lean: function-level with `--span class` flag.)
- Cache key for `grep`: `(prefix, pattern_hash)` vs `(prefix, file_hash,
  pattern_hash)`. The former is cheaper and probably fine.
- For S3 Files (NFS), should we route through the `Store` trait or treat it
  as a local FS shortcut? (Lean: detect and shortcut; the perf gap is too
  big to ignore.)
- Bench-corpus license. SWE-bench has a permissive split; check before we
  commit data.

## Out of scope for v1

- Distributed query (multi-node) — single-binary scale-up first.
- Embedding training or fine-tuning.
- Authn / authz beyond IAM passthrough.
- A hosted product.
