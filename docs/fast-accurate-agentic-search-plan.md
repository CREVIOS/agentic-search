# Plan: Fast, Accurate, Harness-Ready Agentic Search

**Generated**: 2026-05-17  
**Estimated Complexity**: High

## Overview

Make `agentic-search` the fastest and most accurate backend for agent file/search tools by treating it as an agent tool system first, not only a search engine. The default path should stay code-agent optimized: linked ripgrep, AST spans, S3/object-store awareness, cache, and server-side fan-out. Optional indexed stages should be enabled only when they improve a measured workload.

The plan is based on current repo state plus web research:

- MCP tools should return structured content and, for compatibility, mirror it as text; output schemas improve validation and tool parsing.
  Source: https://modelcontextprotocol.io/specification/2025-11-25/server/tools
- Effective agent tools need strong descriptions, focused scopes, meaningful context, token-efficient responses, helpful errors, and eval loops.
  Source: https://www.anthropic.com/engineering/writing-tools-for-agents
- Parallel subagents and parallel tool use help breadth-first search and compression when the task naturally decomposes.
  Source: https://www.anthropic.com/engineering/multi-agent-research-system
- OpenAI Agents SDK, LangChain, and other harnesses expect function/MCP tools with schemas, timeouts, error behavior, and sometimes dynamic tool loading.
  Sources: https://openai.github.io/openai-agents-python/tools/ and https://docs.langchain.com/oss/python/langchain/agents
- Ripgrep is still the right lexical hot path for code because it combines fast regex search, parallelism, and smart filtering.
  Source: https://burntsushi.net/ripgrep/
- RRF remains a practical way to fuse multiple ranked result sets from parallel search methods.
  Source: https://learn.microsoft.com/en-us/azure/search/hybrid-search-ranking
- Tree-sitter is appropriate for span widening because it is fast, robust, and embeddable.
  Source: https://github.com/tree-sitter/tree-sitter

## Prerequisites

- Keep the current Rust workspace green with `cargo test --workspace`.
- Keep Node SDK type generation green with `pnpm --dir sdks/node/agentic-search build`.
- Have a stable RustFS/MinIO runner or AWS S3 test bucket for cold/warm object-store benchmarks.
- Have API keys available only for optional web/e2e harness tests.
- Agree that benchmark thresholds gate “fastest/best” claims in docs.

## Success Targets

- Local grep parity: `agentic-search grep` within 5% of `rg` on local code corpora, or documented exceptions with a measured reason.
- S3 mixed grep: p50 below 800 ms on a 1 GB prefix after manifest work; warm NVMe below 60 ms; warm memory below 12 ms.
- Agent harness success: Claude Agent SDK, OpenAI Agents SDK, LangChain, CrewAI, DeepAgents, and raw MCP can all call the same tools and parse the same structured result shape.
- Accuracy: `find_symbol` precision above 95% on a multi-language symbol suite; NL code search recall@10 materially above lexical baseline by enabling vector/BM25/rerank only where measured.
- Token economy: default tool outputs stay below a configured token budget, with `response_format` and pagination/range controls.

## Sprint 1: Baseline And Contract Lock

**Goal**: Freeze the tool contract and establish honest baselines before optimizing.

**Demo/Validation**:

- `cargo test --workspace`
- `pnpm --dir sdks/node/agentic-search build`
- `python bench/macro/run.py --suite local-grep`
- `python bench/macro/run.py --suite s3-rustfs`

### Task 1.1: Add Golden Tool Schemas

- **Location**: `crates/as-server/src/mcp_stdio.rs`, `crates/as-server/src/handlers.rs`, `sdks/node/agentic-search/src/index.ts`, `sdks/python/*`
- **Description**: Add explicit input and output schemas for `ls`, `read`, `grep`, `find_symbol`, `search`, `index`, `query`, `web`, and `delegate`. Keep MCP `structuredContent` plus mirrored JSON text.
- **Dependencies**: None
- **Acceptance Criteria**:
  - MCP manifest includes output schemas for all tools.
  - REST and SDK types match the MCP schemas.
  - Schema snapshots are checked in.
- **Validation**:
  - Add schema snapshot tests under `crates/as-server/tests/`.

### Task 1.2: Define Stable Span Metadata

- **Location**: `crates/as-grep/src/span.rs`, `crates/as-core/src/lib.rs`
- **Description**: Extend `Span` with `rank_signals`, `source_stage`, `content_hash`, and `truncated` metadata without breaking existing clients.
- **Dependencies**: Task 1.1
- **Acceptance Criteria**:
  - Existing fields remain stable.
  - Clients can understand why a span ranked highly.
- **Validation**:
  - Serde compatibility tests for old and new shapes.

### Task 1.3: Build Baseline Report

- **Location**: `bench/macro/run.py`, `docs/BENCHMARKS.md`, `bench/results/`
- **Description**: Run and record local, RustFS S3, CodeSearchNet lexical, and harness e2e baselines.
- **Dependencies**: None
- **Acceptance Criteria**:
  - Bench output records engine version, git SHA, corpus SHA, runner CPU, memory, and storage type.
  - `docs/BENCHMARKS.md` separates measured numbers from targets.
- **Validation**:
  - CI checks benchmark JSON schema.

## Sprint 2: Hot Path Speed

**Goal**: Make raw search and AST widening approach native `rg` locally while keeping S3/object-store performance first-class.

**Demo/Validation**:

- Local `agentic-search grep` p50 moves toward `rg` parity.
- S3 warm reads demonstrate cache hits without stale results.

### Task 2.1: Local Filesystem Fast Path

- **Location**: `as-store`, `as-fs`, `as-grep`
- **Description**: Add a true `file://` fast path using sync/mmap-compatible reads for local corpora while preserving async object-store reads for S3/GCS/R2.
- **Dependencies**: Task 1.3
- **Acceptance Criteria**:
  - Local benchmark is within 5% of `rg` for grep-only workloads or includes a measured blocker.
  - Object-store behavior is unchanged.
- **Validation**:
  - Local grep parity bench vs `rg`.

### Task 2.2: Prefix Manifest For Cold S3

- **Location**: `as-store`, `as-fs`, `as-cache`, `as-cli`, `as-server`
- **Description**: Add co-located prefix manifests so cold listing collapses from paged `ListObjectsV2` to one manifest GET when available.
- **Dependencies**: Task 1.3
- **Acceptance Criteria**:
  - `agentic-search index-manifest s3://bucket/prefix/` writes a manifest atomically.
  - Search falls back to live listing if the manifest is missing or stale.
- **Validation**:
  - RustFS/S3 cold grep p50 improves and freshness tests pass.

### Task 2.3: AST Parse Cache

- **Location**: `as-ast`, `as-cache`, `as-server/src/handlers.rs`
- **Description**: Cache tree-sitter parse results or extracted container spans by `(uri, etag/content_hash, grammar_version)`.
- **Dependencies**: Task 1.2
- **Acceptance Criteria**:
  - Repeated `grep --ast` over the same prefix avoids reparsing unchanged files.
  - Cache invalidates on content hash or ETag change.
- **Validation**:
  - Benchmark repeated AST search and add invalidation tests.

### Task 2.4: Server-Side Multi-Pattern Fan-Out

- **Location**: `as-plan`, `as-grep`, `as-server/src/handlers.rs`
- **Description**: Let `/search` fan out multiple literal/regex/token probes in one server call, with deadlines and dedup before returning.
- **Dependencies**: Task 1.2
- **Acceptance Criteria**:
  - One `/search` call can run phrase, token OR, filename, and symbol probes concurrently.
  - Slow stages are dropped with stage metadata instead of failing the whole call.
- **Validation**:
  - Planner tests for deadline behavior and deterministic ranking.

## Sprint 3: Accuracy Stack

**Goal**: Improve recall and precision without forcing embeddings onto code-agent hot paths.

**Demo/Validation**:

- CodeSearchNet recall@10 improves over lexical baseline.
- `find_symbol` precision holds above 95%.

### Task 3.1: Query Understanding

- **Location**: `as-plan`, `as-server/src/handlers.rs`
- **Description**: Implement query classification: exact symbol, regex, literal string, natural language code query, document query, and web query.
- **Dependencies**: Task 2.4
- **Acceptance Criteria**:
  - Planner chooses stages based on query type.
  - Agents can override with `mode`.
- **Validation**:
  - Unit tests with query fixtures and expected stage plans.

### Task 3.2: Multi-Language Symbol Index

- **Location**: `as-ast`, `as-index`
- **Description**: Build a lightweight symbol manifest per prefix with definitions, references, imports, exports, language, and byte ranges.
- **Dependencies**: Task 2.3
- **Acceptance Criteria**:
  - `find_symbol` no longer depends on grep hit slack for indexed prefixes.
  - Unsupported languages gracefully fall back to grep+AST.
- **Validation**:
  - Symbol suite across Rust, Python, TS/JS, Go, Java, C/C++.

### Task 3.3: Conditional BM25

- **Location**: `as-index`, `as-plan`
- **Description**: Re-enable Tantivy/BM25 as an optional stage for docs and NL code queries where lexical grep is weak.
- **Dependencies**: Task 3.1
- **Acceptance Criteria**:
  - BM25 is off for simple code grep/symbol tasks.
  - BM25 joins RRF only for classified document/NL queries or explicit `mode`.
- **Validation**:
  - BEIR subset and CodeSearchNet comparisons.

### Task 3.4: Conditional Vector And Rerank

- **Location**: `as-vec`, `as-rerank`, `as-plan`
- **Description**: Use centroid vector search for large unstructured corpora, then optional rerank for top-N only.
- **Dependencies**: Task 3.1, Task 3.3
- **Acceptance Criteria**:
  - Vector path is opt-in or auto-enabled only when an index exists.
  - Rerank has a strict latency budget and never blocks exact-code tasks.
- **Validation**:
  - Recall/latency curves with and without rerank.

## Sprint 4: Agent Harness Compatibility

**Goal**: Make every major agent harness able to use the same tool surface with minimal glue.

**Demo/Validation**:

- Each adapter has a small e2e test that proves the model or harness calls a search tool and parses results.

### Task 4.1: MCP First-Class Server

- **Location**: `crates/as-server/src/mcp_stdio.rs`, `mcp/README.md`
- **Description**: Add resource annotations, tool annotations, output schemas, JSON-RPC conformance tests, notification handling, and streaming-safe responses.
- **Dependencies**: Task 1.1
- **Acceptance Criteria**:
  - Raw MCP clients can list/call tools.
  - Tool schemas pass validation.
- **Validation**:
  - MCP transcript fixture tests.

### Task 4.2: OpenAI Agents SDK Adapter

- **Location**: `sdks/python/openai_agentic_search/` or `sdks/python/agentic_search_tools/`
- **Description**: Add function tools and optional HostedMCPTool config for OpenAI Agents SDK, including timeouts and model-visible error messages.
- **Dependencies**: Task 1.1
- **Acceptance Criteria**:
  - Adapter exposes `search`, `grep`, `find_symbol`, `read`, and `delegate`.
  - Supports `ToolSearchTool`/deferred loading where appropriate.
- **Validation**:
  - Local deterministic tool-call test without model API; optional live test behind env vars.

### Task 4.3: LangChain, CrewAI, DeepAgents Polish

- **Location**: `sdks/python/langchain_agentic_search/`, `sdks/python/crewai_agentic_search/`, `sdks/python/deepagents_search/`
- **Description**: Add structured args, richer docstrings, timeout/error handling, and response formatting controls.
- **Dependencies**: Task 1.1
- **Acceptance Criteria**:
  - Tools can return concise or detailed results.
  - Invalid inputs return actionable errors, not stack traces.
- **Validation**:
  - Existing integration tests plus adapter unit tests.

### Task 4.4: Node And Raw REST Client

- **Location**: `sdks/node/agentic-search/src/index.ts`
- **Description**: Add typed clients for all tools, abort signals, request timeouts, retries for idempotent calls, and streaming hooks.
- **Dependencies**: Task 1.1
- **Acceptance Criteria**:
  - TypeScript SDK exposes every server tool.
  - No schema drift from Rust server.
- **Validation**:
  - `pnpm --dir sdks/node/agentic-search build` and mock HTTP tests.

## Sprint 5: Agentic Planner And Delegate

**Goal**: Give agents fewer, better tools while the server does the parallel search work.

**Demo/Validation**:

- A single `delegate` call answers a broad search task with lower main-context token use than a raw grep/read loop.

### Task 5.1: Planner DSL And Stage Budgets

- **Location**: `as-plan`, `as-server`
- **Description**: Define stage plans with deadlines, max bytes, max files, token budgets, and ranking weights.
- **Dependencies**: Task 3.1
- **Acceptance Criteria**:
  - Plans are serializable in debug output.
  - Stages can be disabled per request or config.
- **Validation**:
  - Unit tests for plan generation and timeout handling.

### Task 5.2: Delegate Endpoint

- **Location**: `as-server`, new `as-delegate` if needed
- **Description**: Implement a search-only subagent loop that can call internal tools in parallel and return compressed findings with citations to spans.
- **Dependencies**: Task 5.1
- **Acceptance Criteria**:
  - Delegate never mutates data.
  - Delegate returns concise findings plus machine-readable span refs.
- **Validation**:
  - Token economy benchmark vs raw agent loop.

### Task 5.3: Result Compression

- **Location**: `as-plan`, `as-server`
- **Description**: Add extractive compression over deduped spans: keep symbol, path, line range, evidence, and next suggested tool call.
- **Dependencies**: Task 1.2, Task 5.1
- **Acceptance Criteria**:
  - `response_format=concise|detailed|jsonl` works across MCP/REST/SDKs.
  - Concise mode returns materially fewer tokens without losing cited evidence.
- **Validation**:
  - Snapshot tests and token-count benchmarks.

## Sprint 6: Continuous Evaluation And Claims Gate

**Goal**: Prevent regressions and only publish “fastest/best” claims when backed by repeatable data.

**Demo/Validation**:

- CI rejects schema drift and correctness regressions.
- Nightly macro benchmarks update a report.

### Task 6.1: Harness Eval Suite

- **Location**: `integration_tests/`, `bench/macro/`, `bench/results/`
- **Description**: Build realistic tool-use tasks for Claude Agent SDK, OpenAI Agents SDK, LangChain, CrewAI, DeepAgents, raw MCP, and raw REST.
- **Dependencies**: Sprint 4
- **Acceptance Criteria**:
  - Each harness test records tool calls, final answer accuracy, latency, and token output size.
  - Live model tests are opt-in behind env vars.
- **Validation**:
  - Offline deterministic tests pass in CI.

### Task 6.2: Accuracy Datasets

- **Location**: `bench/global/`, `bench/fixtures/`
- **Description**: Add symbol lookup fixtures, SWE-bench retrieval slice, CodeSearchNet NL→code, and BEIR doc slices.
- **Dependencies**: Sprint 3
- **Acceptance Criteria**:
  - Each dataset has licensing notes and reproducible preparation scripts.
  - Metrics include precision, recall@k, MRR, NDCG, and “patch touched span found”.
- **Validation**:
  - Dataset prep smoke tests.

### Task 6.3: Perf Regression Gates

- **Location**: `.github/workflows/`, `bench/`, `docs/BENCHMARKS.md`
- **Description**: Add microbench CI gates and nightly macrobench reports.
- **Dependencies**: Task 1.3
- **Acceptance Criteria**:
  - PRs fail on obvious local regressions.
  - Nightly runs publish trend data but do not block normal PRs.
- **Validation**:
  - Simulated regression test in CI.

## Testing Strategy

- **Unit**: Rust tests for schema serialization, query classification, cache freshness, AST span widening, RRF fusion, and planner deadlines.
- **Integration**: MCP JSON-RPC transcript tests, REST tests, SDK mock server tests, RustFS S3 tests.
- **Harness E2E**: Claude Agent SDK, OpenAI Agents SDK, LangChain, CrewAI, DeepAgents, raw MCP, raw REST.
- **Benchmark**: local grep parity, S3 cold/warm, AST cache, CodeSearchNet, BEIR, SWE-bench retrieval slice, token economy.
- **Compatibility**: schema snapshot tests to prevent Rust/MCP/Python/Node drift.

## Potential Risks And Gotchas

- “Fastest” is workload-specific. Mitigation: publish targets and measured numbers separately, and name the corpus/runner.
- Vector search can hurt code tasks. Mitigation: classify queries and keep vector off unless indexed and beneficial.
- Too many tools confuse agents. Mitigation: expose a small core tool set by default and use namespaces/deferred loading for advanced tools.
- S3 listing latency can dominate. Mitigation: prefix manifests, range coalescing, and cache metrics.
- AST grammars vary by language. Mitigation: language support matrix and plain grep fallback.
- Reranking can improve accuracy but blow latency budgets. Mitigation: top-N only, explicit budget, optional mode.
- SDKs drift quickly. Mitigation: adapter contract tests and pinned minimum versions.

## Rollback Plan

- Keep the current grep/read/glob/find_symbol tools stable while adding optional fields and stages.
- Gate new ranking stages behind request flags or config until benchmarks pass.
- If a stage regresses latency or accuracy, disable it in `as-plan` without changing the public tool contract.
- Revert adapter changes independently because SDK wrappers should remain thin over REST/MCP.

## Open Questions

1. Should the default public claim prioritize code repositories, million-document S3 corpora, or both with separate benchmark tables?
2. Which agent harnesses are launch blockers: Claude Agent SDK, OpenAI Agents SDK, LangChain, CrewAI, DeepAgents, Cursor/Cline via MCP, or all of them?
3. Can we use a real AWS S3 runner for public benchmark claims, or should the first published numbers stay on RustFS/MinIO until cloud infra is stable?
