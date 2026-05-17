# Claude Agent SDK + agentic-search — live transcript

Captured on 2026-05-17 from `examples/claude_agent_corpus.py` against
the 4 MB markdown corpus built by `examples/corpus/build.sh` (Rust
Book + Kubernetes concepts + Tokio tutorial).

**Model:** `claude-opus-4-7`
**MCP server:** `agentic-search serve --mcp` (local stdio)
**Tools allowed:** `ls`, `read`, `grep`, `find_symbol`, `search`

## Prompt

> Across this corpus, compare how the Rust Book, the Tokio tutorial,
> and the Kubernetes concepts docs each handle the topic of *graceful
> shutdown* (Rust process exit, Tokio runtime shutdown, k8s pod
> termination). For each, cite one exact filename and one short
> quoted passage. End with a two-sentence synthesis of what is common
> across all three.

## Tool chain (9 MCP calls)

| # | tool | input (truncated) |
|---|------|-------------------|
| 1 | `ls` | `file:///…/examples/corpus/data` |
| 2 | `grep` | `rust-book/ch21-03-graceful-shutdown-and-cleanup.md`, pattern: `shutdown` |
| 3 | `grep` | `tokio-tutorial/`, pattern: `shutdown` |
| 4 | `grep` | `k8s-concepts/workloads/pods/pod-lifecycle.md`, pattern: `…` |
| 5 | `grep` | `tokio-tutorial/`, pattern: `shutdown\|graceful\|drop` |
| 6 | `read` | `tokio-tutorial/select.md`, offset=45, length=400 |
| 7 | `grep` | `tokio-tutorial/select.md`, pattern: `cancellation` |
| 8 | `grep` | `k8s-concepts/.../pod-lifecycle.md`, pattern: `terminationGracePeriodSeconds` |
| 9 | `grep` | `rust-book/ch21-03-graceful-shutdown-and-cleanup.md`, pattern: `Drop` |

## Final answer

> **Rust Book** — `rust-book/ch21-03-graceful-shutdown-and-cleanup.md`:
> "Dropping `sender` closes the channel, which indicates no more
> messages will be" sent, then `Drop` on pool joins each worker
> thread.
>
> **Tokio Tutorial** — `tokio-tutorial/select.md`: "With asynchronous
> Rust, cancellation is performed by dropping a future." Runtime
> shutdown = drop the future / task so its destructors fire.
>
> **Kubernetes** — `k8s-concepts/workloads/pods/pod-lifecycle.md`:
> "to attempt to stop the containers in the pod by first sending a
> TERM (aka. SIGTERM) signal," then SIGKILL after
> `terminationGracePeriodSeconds` (default 30s) expires.
>
> **Synthesis**: All three signal intent-to-stop first (close
> channel / drop future / SIGTERM) so workers finish in-flight work
> before destructors or kill take over. Each pairs that soft signal
> with a hard fallback — `join().unwrap()` panic, future-drop
> cancellation, SIGKILL after grace period — so shutdown still
> terminates if cleanup hangs.

## Notes

- **Grounded.** Every cited file path is a real file in the corpus.
  Every quoted passage is a real substring of that file.
- **Cost.** ~52 k cache-read tokens, ~1.1 k cache-write, ~47 output
  tokens for the answer block (multiple intermediate turns).
- **Latency.** Whole run end-to-end ~25 s including MCP server
  spawn, corpus discovery, and 9 grep round-trips.
- The raw `.jsonl` transcript with every message + every tool result
  is at `examples/transcripts/claude_<timestamp>.jsonl`.
