# agentic-search

> The fastest substrate behind an agent's file tools. S3-native `ls / glob /
> read / grep` with tree-sitter spans, parallel fan-out, sub-agent isolation,
> and an MCP server every agent runtime can talk to. Optional vector / web
> search for the cases where they help.

[![CI](https://github.com/CREVIOS/agentic-search/actions/workflows/ci.yml/badge.svg)](https://github.com/CREVIOS/agentic-search/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

## Why this exists

By 2026 the major coding agents — Claude Code, Cursor, Codex CLI, Devin,
Aider, OpenCode, Continue — have all converged on the same retrieval shape:

```text
agent → parallel(grep, glob, read) → tree-sitter spans → reason → repeat
```

No vector index, no embeddings on the hot path, no RAG. Anthropic explicitly
[replaced](https://robertheubanks.substack.com/p/anthropic-replaced-their-rag-pipeline)
their RAG pipeline with this loop after measuring it. See
[`docs/RESEARCH.md`](docs/RESEARCH.md) for the receipts.

The bottleneck moved. It is now the *speed and ergonomics of the file tools*
the agent calls — especially when the agent's "filesystem" is an S3 bucket.

`agentic-search` is the runtime built for that shape.

## What it does

- **S3 is the filesystem.** Works on raw S3, R2, GCS, [Mountpoint for
  S3](https://github.com/awslabs/mountpoint-s3), and the new
  [S3 Files (NFS)](https://aws.amazon.com/blogs/aws/launching-s3-files-making-s3-buckets-accessible-as-file-systems/).
  No local sync step.
- **Ripgrep linked, not exec'd.** `grep-searcher` runs inside the binary; no
  shell escaping, no process startup tax.
- **Tree-sitter spans** ([Probe](https://github.com/probelabs/probe)-style).
  Matches are expanded to whole functions / classes / methods so the agent
  gets context that compiles, not a half-cut chunk.
- **Tier cache** in the spirit of [Turbopuffer](https://turbopuffer.com/):
  object → NVMe LRU → memory. Warm queries get close to local-FS speed.
- **Parallel fan-out + dedup** on the server. The agent makes one call, the
  server issues 12 grep/AST queries in parallel and returns a single
  deduplicated result.
- **Sub-agent `delegate` endpoint.** Search-only subagent runs in its own
  context window and returns a compressed answer (Anthropic's
  [+90% multi-agent finding](https://www.anthropic.com/engineering/multi-agent-research-system)).
- **Optional vector + web** for cases the grep loop doesn't cover. Vector is
  off by default; web defaults to Exa with Brave / Tavily fallback.
- **One binary, every SDK.** MCP server + REST + Python + Node bindings. First-
  party adapters for Claude Agent SDK, DeepAgents, LangChain, CrewAI.

## Status

Pre-alpha. Public from day one. M0 (skeleton + research synthesis + CI) is
in. M1 (`as-store` + `as-fs` + CLI verbs) is up next. See
[`docs/PLAN.md`](docs/PLAN.md) for the milestones and
[`docs/RESEARCH.md`](docs/RESEARCH.md) for why the plan looks the way it
does.

## Install probe / Node SDK with `pnpm`

```bash
# install pnpm if you don't have it yet
corepack enable && corepack use pnpm@10

# install Probe (one of the engines we benchmark against)
pnpm add -g @probelabs/probe

# build the Node SDK in this repo
pnpm install
pnpm -r build
```

## Quickstart (target shape — wiring in progress)

```bash
cargo install --path crates/as-cli

# treat an S3 prefix as a working directory
agentic-search ls    s3://my-corpus/docs/
agentic-search glob  s3://my-corpus/docs/ "**/*.md"
agentic-search grep  s3://my-corpus/         "TODO\\(security\\)"
agentic-search find  s3://my-repo/src/       --symbol verify_jwt

# expose all of the above to any MCP client
agentic-search serve --mcp
```

### Inside the Claude Agent SDK

```python
from claude_agent_sdk import ClaudeAgentOptions, query
from claude_agent_search import mcp_server_config, as_tools

opts = ClaudeAgentOptions(
    mcp_servers=mcp_server_config(),
    allowed_tools=as_tools(),
)

async for msg in query(
    prompt="Find every place we still use HS256 in s3://corp/ and summarize.",
    options=opts,
):
    print(msg)
```

### Inside DeepAgents

```python
from deepagents import create_deep_agent
from deepagents_search import search_tool

agent = create_deep_agent(tools=[search_tool()])
```

## Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│  SDK adapters  ·  Claude Agent SDK  DeepAgents  LangChain  CrewAI│
├──────────────────────────────────────────────────────────────────┤
│  Tool surface  ·  ls  glob  read  grep  find_symbol  search  web │
│                ·  delegate (sub-agent isolation)                 │
├──────────────────────────────────────────────────────────────────┤
│  Planner       ·  parallel fan-out  ·  dedup by span             │
├───────────────┬───────────────┬────────────────┬─────────────────┤
│  Grep         │  AST          │  Optional      │  Web            │
│  (rg-as-lib)  │  (tree-sitter)│  vector (off)  │  (Exa/Brave/    │
│               │               │                │   Tavily)       │
├───────────────┴───────────────┴────────────────┴─────────────────┤
│  Tier cache   ·  memory LRU  →  NVMe LRU  →  manifests           │
├──────────────────────────────────────────────────────────────────┤
│  Object store ·  s3 · r2 · gcs · s3-files · mountpoint · file    │
└──────────────────────────────────────────────────────────────────┘
```

Crate-level breakdown lives in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Benchmarks

We target three numbers above all:

1. **Warm S3 grep < 60 ms** on a 1 GB prefix.
2. **Cold S3 grep < 800 ms** on a 1 GB prefix.
3. **Within 5% of native ripgrep** on identical local corpora.

Plus parallel-fan-out scaling (12 grep queries should land in ≤1.25× the time
of one), `find_symbol` recall on a coding-agent trace suite, and
`delegate(query)` token economy vs. raw agent loops. Full numbers in
[`docs/BENCHMARKS.md`](docs/BENCHMARKS.md) once M6 lands.

## License

Apache-2.0. See [LICENSE](LICENSE).
