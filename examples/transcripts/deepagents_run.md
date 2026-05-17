# DeepAgents + agentic-search REST (over real S3) — live transcript

Captured on 2026-05-17 from `examples/deepagents_corpus.py` against the
**RustFS-backed S3 bucket** `s3://agentic-search-it/corpus` (297 .md
files, 3.5 MB, real SigV4-signed S3 wire protocol).

**Model:** `claude-sonnet-4-6` (default DeepAgents model)
**Server:** `target/release/agentic-search serve` with
  `AWS_ENDPOINT_URL=http://localhost:19000` +
  `AWS_ACCESS_KEY_ID/SECRET_ACCESS_KEY` pointing at the RustFS
  container; every request out of the server is a real S3 API call.
**Tools allowed:** `search`, `read_file` (REST against the local
  agentic-search server).

## Prompt

> Corpus root: `s3://agentic-search-it/corpus`
>
> Find every place in the corpus that discusses *backpressure* or
> *rate limiting* between async producers and consumers. Cite at
> most 3 distinct files, one short quoted passage each. Then in
> two sentences synthesise the trade-off they all warn about. Use
> the `search` tool first, `read_file` only if a snippet is too
> short to be quotable.

## Final answer

> ### Files Discussing Backpressure / Rate Limiting Between Async Producers and Consumers
>
> #### 1. `corpus/tokio-tutorial/channels.md`
> > *"The channel is created with a capacity of 32. If messages are
> > sent faster than they are received, the channel will store them.
> > Once the 32 messages are stored in the channel, calling
> > `send(...).await` will go to sleep until a message has been
> > removed by the receiver."*
>
> #### 2. `corpus/rust-book/ch17-02-concurrency-with-async.md` (line 220)
> > *"…because the channel we're sending it into is unbounded."*
>
> #### 3. `corpus/k8s-concepts/cluster-administration/flow-control.md` (line 24)
> > *"…a limited amount of queuing, so that no requests are rejected
> > in cases \[of brief load spikes\]…"*
>
> ### Synthesis
>
> All three sources warn that **unbounded queuing shifts the
> pressure problem from the producer to memory**: the producer
> never stalls, so bursts are silently absorbed until the system
> runs out of resources rather than signalling the caller to slow
> down. The core trade-off is therefore **throughput/latency for
> producers vs. resource exhaustion and loss of flow-control
> feedback** — bounded queues (or concurrency limits) force
> producers to block or be rejected early, preserving system
> stability at the cost of higher per-request latency under load.

## Notes

- **Real S3 protocol.** Every grep / read call the server made
  resolved into a signed S3 `GET` against RustFS (`http://localhost
  :19000`). The agent did not touch the filesystem directly — only
  the agentic-search REST endpoint, which then signed S3 requests
  with its own AWS env.
- **Grounded across 3 corpora.** Tokio's tutorial, the Rust Book,
  and Kubernetes' API Priority and Fairness docs all show up in
  the answer with line-accurate citations.
- **Cost.** ~29 k cache-read tokens, ~1 k cache-write, ~536 output
  tokens.
- Full LangGraph state dump in
  `examples/transcripts/deepagents_<timestamp>.jsonl`.
