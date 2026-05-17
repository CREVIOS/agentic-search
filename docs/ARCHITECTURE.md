# Architecture

## Layers (top → bottom)

1. **SDK adapters** — language-native wrappers per agent runtime.
2. **Server surface** — MCP (stdio + http), REST (axum), gRPC (tonic, M5+).
3. **Planner** — query parsing, route selection (lex/vec/web/grep), fusion
   (RRF), reranking, budget enforcement.
4. **Indexes** — tantivy (lex), HNSW (vec), web providers.
5. **FS** — virtual filesystem (`Fs`) over `Store`.
6. **Store** — `s3://`, `gs://`, `r2://`, `file://` behind one trait.
7. **Cache** — NVMe LRU for hot ranges + manifests.

## Hot-path principles

- **No subprocess for grep.** `grep-searcher` is linked in.
- **No double allocation on read.** Bytes flow from `aws-sdk-s3` → `Bytes` →
  searcher slice. No string copy unless the caller asks for a snippet.
- **Async everywhere except the inner search loop.** Tantivy and ripgrep are
  CPU-bound; we drive them on a Rayon pool and bridge with `spawn_blocking`.
- **Manifests co-locate with data.** A small `_manifest.parquet` per prefix
  lets a cold listing return in one S3 GET rather than paging `ListObjects`.

## Indexing

- Documents arrive via CLI (`agentic-search index`), HTTP `/index`, or push API.
- Each batch builds a tantivy segment + an HNSW shard locally, then uploads to
  the object store under `s3://bucket/.index/{segment_id}/`.
- A manifest (`_manifest.parquet`) tracks segments, doc counts, and which
  HNSW shard owns which vectors.
- Realtime updates land in an in-memory delta index + WAL; a background task
  compacts to S3 segments.

## Query path

```
agent → tool call
      ↓
   planner
   ├─ lex search   (tantivy)            ─┐
   ├─ vec search   (HNSW + embed)        │  → RRF fuse → top-50
   ├─ web search   (provider)            │
   └─ grep         (range-scan)         ─┘
                                          ↓
                                       rerank top-N (optional)
                                          ↓
                                       return top-k
```

## Caching

- LRU keyed by `(store, key, range)` on NVMe.
- Manifests are always cached; they are tiny and queried per-call.
- Embedding cache keyed by `(model, text_hash)` lives in the same LRU.

## Realtime

- WAL append on every write, flushed every N ms or M bytes.
- In-memory delta tantivy index serves queries alongside the cold segments.
- Compactor merges delta → new S3 segment, advances the manifest atomically.

## Failure model

- All store ops are retried with exponential backoff via the `object_store`
  crate's built-in retrier.
- Index ops are idempotent on `doc.id`; reindexing the same doc replaces it.
- Planner stages run with deadlines; a stage that misses its budget is dropped
  from fusion rather than failing the whole query.
