# Changelog

All notable changes to `agentic-search` are documented here. The format
follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] — 2026-05-17

First public release. Core agent-loop search path is production-ready.

### Added

- **Rust workspace** (13 crates): `as-core`, `as-store`, `as-fs`, `as-grep`,
  `as-ast`, `as-cache`, `as-embed`, `as-vec`, `as-web`, `as-rerank`,
  `as-plan`, `as-server`, `as-cli`, plus `bench/`.
- **CLI** (`agentic-search`) with `ls / glob / read / grep / find / serve /
  index-manifest / index / query` verbs.
- **MCP stdio server** following JSON-RPC 2.0 (protocol 2025-11-25) with
  `inputSchema` + `outputSchema` per tool; notification handling; unknown-
  method response with code `-32601`.
- **REST server** (`/grep`, `/search`, `/read`, `/delegate`, …) bound to
  `127.0.0.1` by default; `--allow-public` required for non-loopback bind.
- **Object-store backends**: S3, R2, GCS via `object_store 0.13`; local
  `LocalMmapStore` (memmap2 + `Bytes::from_owner`) with path-escape
  rejection.
- **Tier cache** (`as-cache`): in-memory LRU (`parking_lot`) → NVMe LRU
  with mtime sweep every 64 writes → object store. Cache key includes
  store identity to prevent cross-bucket collisions.
- **Tree-sitter spans** (`as-ast`): `ContainerIndex` (parse-once per file,
  `partition_point` lookup), `SpanCache` keyed by
  `(GRAMMAR_VERSION, lang_id, content_hash)`, `widen_with_cache_cancellable`
  for cooperative cancel.
- **Centroid vector index** (`as-vec`, Turbopuffer-shaped): k-means
  clusters live on object storage, queries are 2 roundtrips, cluster cache
  is an `LruCache(4096)`.
- **fastembed-rs** (`as-embed`): BGE-small-en-v1.5 (384d ONNX).
- **Planner** (`as-plan`): parallel fan-out via `tokio::JoinSet`, per-stage
  budgets, RRF (k=60) fusion preserving `source_stage`.
- **Cold-S3 manifest**: gzipped JSONL `.agentic-search/manifest.jsonl.gz`;
  streaming reader tolerates mid-line corruption.
- **SDK adapters** (Python): `claude-agent-search`, `openai-agentic-search`,
  `deepagents-search`, `langchain-agentic-search`, `crewai-agentic-search`.
- **Node SDK** (`@agentic-search/sdk`): TypeScript REST client.
- **Go SDK** (`github.com/CREVIOS/agentic-search/sdks/go/agenticsearch`):
  REST client with context cancellation.
- **Bench harness**: `bench/macro/run.py` (server + CLI shapes) and
  `bench/global/codesearchnet.py` (CodeSearchNet slice).
- **Local-S3 testing**: `docker-compose.rustfs.yml` + `scripts/rustfs-up.sh`.
- **CI**: clippy `-D warnings` gate, criterion regression, `cargo-deny`,
  `gitleaks`.

### Performance

- Server-shape `/grep` p50 **16.4 ms** (faster than native `rg` at 17.3 ms
  on the same 782-file Rust corpus) while emitting JSON spans and keeping
  a tier cache.
- Server-shape `/grep` with `ast: true` warm cache: p50 **31.7 ms** — 13×
  faster than CLI cold path (579 ms). Wins from content-addressed
  `SpanCache` so vendored files share one parse, mmap fast path for
  `file://`, NVMe LRU sweep keeping disk bounded, `JoinSet` cancelling
  detached AST tasks on early return.

### Security

- Loopback bind by default; explicit `--allow-public` flag required to
  bind a non-loopback address.
- `..`-segment rejection in `LocalMmapStore::safe_path`.
- `cargo-deny` advisories/bans/licenses/sources allowlist (no transitive
  surprises).
- `gitleaks` action wired into the security workflow.

[0.1.0]: https://github.com/CREVIOS/agentic-search/releases/tag/v0.1.0
