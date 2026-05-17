# Node & Go SDKs — live transcript (real S3 via RustFS)

Both SDKs hit the same `agentic-search serve` REST endpoint at
`http://127.0.0.1:8787`. The server in turn signs SigV4 requests
against the local RustFS container (`http://localhost:19000`),
exercising the **real S3 wire protocol** end-to-end — no `file://`
shortcuts.

Corpus: `s3://agentic-search-it/corpus` (297 .md files synced from
the Rust Book + Tokio tutorial + Kubernetes concepts, ~3.5 MB).

## Node / TypeScript (`examples/node_corpus.ts`)

```
== /health (http://127.0.0.1:8787) ==
  200 OK

== /grep s3://agentic-search-it/corpus for "graceful shutdown" (top 5) ==
  corpus/k8s-concepts/cluster-administration/node-shutdown.md:16   The `unattended-upgrades` package from Debian conflicts with…
  corpus/k8s-concepts/cluster-administration/node-shutdown.md:107  During a graceful shutdown, kubelet terminates pods in two phases:
  corpus/k8s-concepts/cluster-administration/node-shutdown.md:281  During a non-graceful shutdown, Pods are terminated in the two…
  corpus/k8s-concepts/workloads/pods/pod-lifecycle.md:1001         …terminating (a graceful shutdown duration has been set), the kubelet…
  corpus/k8s-concepts/workloads/pods/pod-lifecycle.md:1026         At the same time as the kubelet is starting graceful shutdown…

== /find_symbol s3://agentic-search-it/corpus symbol "verify_jwt" (max 3) ==
  0 hits (expected 0 — corpus is markdown, no code symbols)

== /search s3://agentic-search-it/corpus "backpressure unbounded queue" k=3 ==
  corpus/k8s-concepts/cluster-administration/dra.md:121      * Workqueue Add Rate: Monitor…
  corpus/tokio-tutorial/channels.md:430                      # Backpressure and bounded channels
  corpus/k8s-concepts/cluster-administration/dra.md:122      * Workqueue Depth: Track…
```

## Go (`examples/go_corpus/main.go`)

Identical output to the Node run above — both SDKs serialize the same
JSON shape on the wire. Confirms:

1. `AgenticSearchClient.grep` (TS) and `Client.Grep` (Go) produce
   byte-identical span lists against the same corpus.
2. `findSymbol` (TS) and `FindSymbol` (Go) both POST `/find` (not
   `/find_symbol`) — the codex-round-3 P1 fix.
3. The `/read` response decodes `text` (not `content`) in both SDKs.

## Why this matters

The agent loop the Claude Agent SDK and DeepAgents transcripts ran
above — `grep → read → cite` — works identically from a Node service,
a Go service, or any HTTP client. The server is the only thing that
knows about S3 auth; clients pass `s3://bucket/prefix` strings and
get JSON spans back.
