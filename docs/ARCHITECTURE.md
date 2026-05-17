# Architecture

Read [`RESEARCH.md`](RESEARCH.md) and [`PLAN.md`](PLAN.md) first — they
explain *why* the layering looks this way. This document explains *what* is
in each layer.

## Layers (top → bottom)

1. **SDK adapters** — Claude Agent SDK, DeepAgents, LangChain, CrewAI, Node.
2. **Server surface** — MCP (stdio + http), REST (axum), gRPC (M6+).
3. **Tool surface** — `ls`, `glob`, `read`, `grep`, `find_symbol`, `search`,
   `web`, `delegate`. All streaming, all parallel-safe.
4. **Planner** — parses the tool call, expands it into parallel sub-searches
   (`as-plan`), dedups by span, fuses with RRF where multiple stages run.
5. **Search stages**:
   - `as-grep` — `grep-searcher` linked in; range-coalesces S3 reads.
   - `as-ast` — tree-sitter spans (function/class/method).
   - `as-index` — optional tantivy BM25 for unstructured docs.
   - `as-vec` — opt-in fastembed + HNSW for unstructured corpora.
   - `as-web` — Exa default, Brave / Tavily fallback.
6. **Cache** — `as-cache`: memory LRU in front of NVMe LRU in front of the
   object store. Keys: `(store, key, range)` for file ranges,
   `(prefix, pattern_hash)` for grep results, `(prefix, file_hash)` for AST
   trees.
7. **Filesystem** — `as-fs`: POSIX-ish operations over `as-store`. Detects
   when the underlying mount is S3 Files / Mountpoint and shortcuts via the
   local FS path.
8. **Store** — `as-store`: `s3://`, `r2://`, `gs://`, `file://` behind one
   trait, powered by `aws-sdk-s3` and the `object_store` crate.

## Hot-path principles

- **No subprocess for grep.** `grep-searcher` is linked in.
- **No double allocation on read.** Bytes flow `s3 → Bytes → searcher
  slice`. No string copy unless the caller asks for a snippet.
- **Async everywhere except the CPU-bound inner loop.** Tantivy, ripgrep and
  tree-sitter run on a Rayon pool; the async layer bridges with
  `spawn_blocking`.
- **Manifests co-locate with data.** A small `_manifest.parquet` per prefix
  lets a cold listing return in one S3 GET rather than paging
  `ListObjects`.
- **Parallel fan-out on the server.** When the planner expands one tool
  call into N searches, those N searches run concurrently and the server
  returns one deduplicated block — the agent never has to orchestrate.

## Span model

A search hit is **never** a raw text chunk. It is:

```rust
struct Span {
    uri: String,           // s3://bucket/key
    range: Range<u64>,     // byte range
    line_range: [u32; 2],  // 1-based, inclusive
    symbol: Option<String>,// function/class/method name (when AST hits)
    kind: SpanKind,        // Function | Class | Method | Block | Line
    snippet: String,       // up to N tokens, span-aligned
    score: f32,
}
```

The agent receives spans, not lines. Spans are deduplicated across stages —
if both `grep` and `find_symbol` hit the same function, only one span is
returned (with the union of evidence in `metadata`).

## Caching

The cache has three explicit layers:

1. **Memory LRU** — bounded by config; holds the hottest spans and manifest
   slices.
2. **NVMe LRU** — `foyer` or hand-rolled; bounded by disk budget; holds
   range bytes, AST trees, BM25 segments.
3. **Object store** — source of truth; everything is reproducible from here.

The object store also holds *indexed* artifacts (manifests, tantivy
segments, HNSW shards) when the user has run `agentic-search index`. The
index lives next to the data; nothing is ever stranded in a local cache.

## Realtime / freshness

For raw `grep`/`glob` over a prefix, freshness is trivially S3-fresh — we
list-on-demand and the cache invalidates on `ETag` change.

For indexed stages (`as-index`, `as-vec`):

- Writes append to a WAL and to an in-memory delta segment.
- The delta segment is queried *alongside* the cold segments.
- A compactor merges delta → new S3 segment and atomically advances the
  manifest pointer.

## Failure model

- Store ops are retried with exponential backoff (via `object_store` /
  `aws-sdk-s3`).
- Index ops are idempotent on `(uri, byte_range)`; replays do not duplicate.
- Planner stages run with deadlines; a stage that misses its budget is
  dropped from fusion rather than failing the whole query. This is the same
  pattern the Anthropic multi-agent research system uses for subagents.

## What is *not* in this architecture

- A dedicated rerank model on the default path. Cross-encoder rerank is
  available via `as-rerank` but the planner only invokes it when the user
  enables `--rerank` or the workload is non-code.
- Embedding training / fine-tuning.
- A control plane (multi-tenant auth, billing). v1 ships single-binary
  scale-up; we revisit at v2.
