# Research synthesis (May 2026)

This is the landscape `agentic-search` is built into. Every design choice in
[`PLAN.md`](PLAN.md) traces back to one of these findings.

## What "agentic search" actually means

Until ~2024 most teams equated retrieval with RAG: chunk → embed → ANN. By
2026 the largest coding-agent vendors have walked away from that pattern for
file/code workloads.

- **Anthropic**: Claude Code originally shipped a RAG pipeline with embeddings
  and a vector DB. The team tested an alternative — `grep + glob + read` with
  iterative reasoning — and it "outperformed everything, by a lot." RAG was
  dropped. Source: Boris Cherny (Claude Code creator), summarised in
  [_Why Cursor, Claude Code, and Devin use grep, not vectors_](https://www.mindstudio.ai/blog/is-rag-dead-what-ai-agents-use-instead)
  and [_Anthropic replaced their RAG pipeline with agentic search_](https://robertheubanks.substack.com/p/anthropic-replaced-their-rag-pipeline).
- **Cursor / Codex CLI / OpenCode / Aider / Continue / Devin** all default to
  ripgrep-style scanning, not vector retrieval. ([Why Coding Agents Still Use
  grep](https://yage.ai/share/why-coding-agents-still-use-grep-en-20260327.html),
  [Morph: Agentic Search](https://www.morphllm.com/agentic-search))
- **Claude Code, April 2026** swapped ripgrep for `ugrep` + `bfs` on native
  builds to shave another factor in latency — the trend is **faster** grep,
  not less grep. ([How Claude Code Actually Works](https://www.buildberg.co/blog/claude-code-complete-guide))

Reasons the agentic loop wins for code:

1. **Precision**: regex matches are exact; embeddings hallucinate near-misses.
2. **Freshness**: a pre-built index drifts during active editing.
3. **No index to maintain**: zero cold-start cost for a new repo.
4. **Privacy**: no embedding RPCs.
5. **Spans, not chunks**: with AST awareness, the agent gets whole functions.

This is the workload `agentic-search` optimizes for. Vector retrieval is still
useful on non-code, unstructured corpora — and we ship it — but it is **not**
the default path.

## What the agent runtimes already provide

- **Claude Agent SDK**: ships `Read`, `Write`, `Edit`, `Bash`, `Glob`, `Grep`,
  `WebSearch`, `WebFetch`, `AskUserQuestion`. Read-only tools run in parallel
  inside a single turn. ([Agent SDK overview](https://code.claude.com/docs/en/agent-sdk/overview))
- **DeepAgents** (LangChain): `FilesystemMiddleware` ships `ls`, `read_file`,
  `write_file`, `edit_file`, `glob`, `grep` over a `BackendProtocol` so the
  filesystem can be local, Modal, or Daytona. ([DeepAgents architecture](https://eastondev.com/blog/en/posts/ai/deepagents-architecture/))
- The **shape** of the tool surface is converged. Our job is to make the
  backend behind that shape **faster and S3-native** without forcing anyone to
  rewrite their agent.

## Closest open-source comp: Probe

[`probelabs/probe`](https://github.com/probelabs/probe) is the strongest piece
of prior art: ripgrep + tree-sitter AST, returns whole functions/classes, runs
fully local. Probe is the right pattern.

What Probe doesn't do:

- **S3 / object storage** — local FS only.
- **Multi-tenant** — one repo per process.
- **Server / MCP / SDK adapters** — CLI-first.
- **Web search** — out of scope.
- **Cache tiering** — relies on OS page cache.

`agentic-search` keeps Probe's "ripgrep + tree-sitter → spans" backbone and
adds the S3-native runtime, the cache tier, and the SDK surface.

## Object-storage-native designs we are learning from

### Turbopuffer (vector, but the tier model is the lesson)

Turbopuffer serves trillions of vectors with S3/GCS/Azure as source of truth,
NVMe SSD as warm cache, RAM as hot cache. "JIT-compiler-like": queries warm
the cache, and the more you query, the closer data moves to the CPU. Cost is
~95% lower than always-on cluster DBs. ([turbopuffer.com/docs/architecture](https://turbopuffer.com/docs/architecture),
[How Turbopuffer Serves 2.5T Vectors on S3](https://ajay-edupuganti.medium.com/how-turbopuffer-serves-2-5-trillion-vectors-on-s3-7d7ab7f9a7fa))

**Takeaway for us**: copy the *tier model* (object → NVMe → memory) but apply
it to file ranges and grep indexes, not vectors.

### Pinecone serverless

Slab-based architecture: vectors live full-fidelity on S3 in immutable slabs,
metadata bitmaps in the same slabs, compute autoscales independently.
([Pinecone serverless architecture](https://docs.pinecone.io/reference/architecture/serverless-architecture))

**Takeaway**: immutable, content-addressed segments on object storage make
range reads, caching, and concurrent writers tractable. Apply the same to
tantivy segments and tree-sitter span indexes.

### ChromaDB

Cloud variant uses memory/SSD/object-store tiering similar to Turbopuffer.
Local variant is SQLite + HNSW on disk. S3 backend has been an open feature
request since 2024 ([chroma-core/chroma#1736](https://github.com/chroma-core/chroma/issues/1736));
adoption from the team has only landed via the hosted product.

**Takeaway**: there is still no popular open-source vector layer that is
S3-native and tier-cached. The vector layer we ship is optional but, when
enabled, it should follow Turbopuffer's pattern, not Chroma local's.

## The new S3 substrate (April 2026)

[Amazon S3 Files](https://aws.amazon.com/blogs/aws/launching-s3-files-making-s3-buckets-accessible-as-file-systems/)
turns a bucket into NFSv4.1 with full file-system semantics — created
explicitly so that agents like Claude Code and Kiro can treat a bucket as a
working directory. ([VentureBeat: S3 Files gives AI agents a native file
system workspace](https://venturebeat.com/data/amazon-s3-files-gives-ai-agents-a-native-file-system-workspace-ending-the))

[Mountpoint for S3](https://github.com/awslabs/mountpoint-s3) is the existing
high-throughput read-mostly file client.

**Takeaway**: `agentic-search` must work on raw S3, S3 Files (NFS), and
Mountpoint with the *same* tool surface, and it must not get worse just
because one of those layers is below it.

## Web-search side

[Exa Instant](https://exa.ai/) hits <200 ms p50 with neural embeddings + parse
to markdown / highlights tuned for token budgets — significantly faster than
wrappers around Google/Bing. Brave and Tavily are the strongest fallbacks for
self-hosters who do not want to depend on Exa.

**Takeaway**: web search is pluggable but Exa is the default in the same
spirit that ripgrep is the default for files.

## Multi-agent / sub-agent isolation

Anthropic's multi-agent research write-up reports a multi-agent system with
Opus + Sonnet subagents outperformed single-agent Opus by 90.2% on internal
research evals. Subagents operate in their own context windows; the lead
agent only ever sees the compressed result. ([How we built our multi-agent
research system](https://www.anthropic.com/engineering/multi-agent-research-system))

**Takeaway**: `agentic-search` exposes a `delegate(query)` endpoint that runs
a search-only subagent loop and returns a compressed answer — so callers can
keep the main context window tight without writing their own orchestration.

## Cline-style three-tier retrieval

[Cline](https://github.com/cline/cline) implements three retrieval layers in
parallel: regex via ripgrep, fuzzy filename/folder search, and AST definition
extraction via tree-sitter.

**Takeaway**: ship all three in one server, fuse with RRF, dedup by span.

## Decision matrix (what we adopt, what we drop)

| Idea                                                  | Adopt? | Why                                                                                   |
| ----------------------------------------------------- | :----: | ------------------------------------------------------------------------------------- |
| Grep + glob + read as primary tool surface            |   ✅   | Industry consensus, Anthropic eval                                                    |
| Ripgrep as a library (no subprocess)                  |   ✅   | Latency, control, no shell escaping                                                   |
| Tree-sitter span extraction (Probe pattern)           |   ✅   | Whole functions/classes, not chunks                                                   |
| S3 as filesystem (raw + S3 Files + Mountpoint)        |   ✅   | The whole reason we exist                                                             |
| Object → NVMe → memory tier cache (Turbopuffer)       |   ✅   | Warm queries get close to local-FS speed                                              |
| Parallel tool dispatch + dedup on the server         |   ✅   | One round-trip from the agent, N searches done                                        |
| Sub-agent `delegate` endpoint                         |   ✅   | Anthropic's +90% finding                                                              |
| MCP + REST + bindings for Python/Node                 |   ✅   | Works with every agent runtime                                                        |
| Web search (Exa default, Brave/Tavily fallback)       |   ✅   | Tight, token-efficient                                                                |
| Vector / embedding index                              |  Opt-in | Anthropic disproved it as default; still useful for non-code/PDF                       |
| Heavy cross-encoder rerank in default path            |   ❌   | Cost vs. payoff for code search is poor                                               |
| RAG-style fixed chunking                              |   ❌   | Span-aware retrieval supersedes it                                                    |

## Sources

- [Why Coding Agents Still Use grep](https://yage.ai/share/why-coding-agents-still-use-grep-en-20260327.html)
- [Why Cursor, Claude Code, and Devin use grep, not vectors](https://www.mindstudio.ai/blog/is-rag-dead-what-ai-agents-use-instead)
- [Claude Code Doesn't Index Your Codebase](https://vadim.blog/claude-code-no-indexing)
- [Morph: Agentic Search](https://www.morphllm.com/agentic-search)
- [Anthropic: Effective context engineering for AI agents](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)
- [Anthropic: How we built our multi-agent research system](https://www.anthropic.com/engineering/multi-agent-research-system)
- [Anthropic Replaced Their RAG Pipeline with Agentic Search](https://robertheubanks.substack.com/p/anthropic-replaced-their-rag-pipeline)
- [Claude Agent SDK overview](https://code.claude.com/docs/en/agent-sdk/overview)
- [DeepAgents architecture](https://eastondev.com/blog/en/posts/ai/deepagents-architecture/)
- [DeepAgents Filesystem Operations](https://deepwiki.com/langchain-ai/deepagents/2.5-filesystem-operations)
- [Probe (ripgrep + tree-sitter)](https://github.com/probelabs/probe)
- [Turbopuffer architecture](https://turbopuffer.com/docs/architecture)
- [How Turbopuffer Serves 2.5T Vectors on S3](https://ajay-edupuganti.medium.com/how-turbopuffer-serves-2-5-trillion-vectors-on-s3-7d7ab7f9a7fa)
- [Pinecone serverless architecture](https://docs.pinecone.io/reference/architecture/serverless-architecture)
- [Chroma Storage Layout](https://cookbook.chromadb.dev/core/storage-layout/)
- [Chroma S3 backend feature request (#1736)](https://github.com/chroma-core/chroma/issues/1736)
- [Amazon S3 Files launch](https://aws.amazon.com/blogs/aws/launching-s3-files-making-s3-buckets-accessible-as-file-systems/)
- [VentureBeat: S3 Files for AI agents](https://venturebeat.com/data/amazon-s3-files-gives-ai-agents-a-native-file-system-workspace-ending-the)
- [Mountpoint for Amazon S3](https://github.com/awslabs/mountpoint-s3)
- [Exa Instant <200ms](https://www.marktechpost.com/2026/02/13/exa-ai-introduces-exa-instant-a-sub-200ms-neural-search-engine-designed-to-eliminate-bottlenecks-for-real-time-agentic-workflows/)
