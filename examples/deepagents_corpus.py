"""DeepAgents + agentic-search REST — real corpus, multi-tool query.

Hits the live `agentic-search serve` HTTP server (default
127.0.0.1:8787) using the `deepagents_search` adapter tools, then
delegates a multi-step research task to a DeepAgent.

Run:
    bash examples/corpus/build.sh
    cargo build --release -p agentic-search-cli
    target/release/agentic-search serve &
    source .venv-examples/bin/activate
    pip install -e sdks/python/deepagents_search   # local adapter
    python examples/deepagents_corpus.py
"""

from __future__ import annotations

import json
import os
import pathlib
import sys
from datetime import datetime

ROOT = pathlib.Path(__file__).resolve().parent.parent
# Default to the RustFS-backed S3 bucket so this exercises the *real*
# S3 wire protocol (SigV4-signed requests against an S3-compatible
# endpoint). Override to `file://…` for a local-only smoke test.
CORPUS_URI = os.environ.get("AGENTIC_SEARCH_CORPUS", "s3://agentic-search-it/corpus")
SERVER_URL = os.environ.get("AGENTIC_SEARCH_URL", "http://127.0.0.1:8787")
TRANSCRIPT = ROOT / "examples" / "transcripts" / f"deepagents_{datetime.utcnow().strftime('%Y%m%dT%H%M%S')}.jsonl"


def load_env() -> None:
    env = ROOT / ".env"
    if not env.exists():
        return
    for line in env.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        k, _, v = line.partition("=")
        os.environ.setdefault(k.strip(), v.strip())
    if not os.environ.get("ANTHROPIC_API_KEY") and os.environ.get("APP_CLAUDE_API_KEY"):
        os.environ["ANTHROPIC_API_KEY"] = os.environ["APP_CLAUDE_API_KEY"]


def main() -> int:
    load_env()

    # The deepagents_search adapter wants the REST server up. Smoke
    # check before instantiating the agent so we fail fast with a
    # clean message.
    import requests
    try:
        r = requests.get(f"{SERVER_URL}/health", timeout=2)
        r.raise_for_status()
    except Exception as e:
        print(f"agentic-search REST server not reachable at {SERVER_URL}: {e}", file=sys.stderr)
        print("  start with: target/release/agentic-search serve", file=sys.stderr)
        return 2

    if not os.environ.get("ANTHROPIC_API_KEY"):
        print("ANTHROPIC_API_KEY missing (or APP_CLAUDE_API_KEY in .env)", file=sys.stderr)
        return 2

    from deepagents import create_deep_agent

    # Pin the corpus URI inside a tool wrapper so the agent can't
    # accidentally point at the wrong prefix. Defaults to the
    # RustFS-backed `s3://agentic-search-it/corpus` (real S3 wire
    # protocol against a local container); set
    # AGENTIC_SEARCH_CORPUS=file:///… to swap in a local FS path.
    corpus_uri = CORPUS_URI

    # Span URIs come back as **prefix-relative keys** (e.g.
    # `corpus/k8s-concepts/.../node-shutdown.md`), not full URIs.
    # Reassemble into the full `s3://bucket/key` before issuing a
    # downstream `/read`.
    def _abs(key: str) -> str:
        if "://" in key:
            return key
        scheme, _, host_path = corpus_uri.partition("://")
        host, _, prefix = host_path.partition("/")
        prefix = prefix.rstrip("/")
        # `key` already includes the prefix segment from grep output,
        # so strip the corpus prefix if it duplicates.
        if prefix and key.startswith(prefix + "/"):
            key = key[len(prefix) + 1 :]
        joined = f"{prefix}/{key}" if prefix else key
        return f"{scheme}://{host}/{joined}"

    def search(query: str, k: int = 8) -> str:
        """Hybrid grep + AST + (optional) vector search over the corpus."""
        r = requests.post(
            f"{SERVER_URL}/search",
            json={"uri": corpus_uri, "query": query, "k": k},
            timeout=30,
        )
        r.raise_for_status()
        return json.dumps(r.json().get("spans", []), default=str)[:8000]

    def read_file(uri: str, length: int = 4000) -> str:
        """Read a file from the corpus by URI (full s3://… or relative key)."""
        full = _abs(uri)
        r = requests.post(
            f"{SERVER_URL}/read",
            json={"uri": full, "offset": 0, "length": length},
            timeout=30,
        )
        r.raise_for_status()
        return (r.json().get("text") or "")[:length]

    agent = create_deep_agent(tools=[search, read_file])

    prompt = (
        f"Corpus root: {corpus_uri}\n\n"
        "Find every place in the corpus that discusses *backpressure* "
        "or *rate limiting* between async producers and consumers. "
        "Cite at most 3 distinct files, one short quoted passage each. "
        "Then in two sentences synthesise the trade-off they all warn "
        "about. Use the `search` tool first, `read_file` only if a "
        "snippet is too short to be quotable."
    )

    print(f"== prompt ==\n{prompt}\n\n== transcript → {TRANSCRIPT.relative_to(ROOT)} ==\n")
    TRANSCRIPT.parent.mkdir(parents=True, exist_ok=True)

    # `create_deep_agent` returns a LangGraph runnable. Invoke and
    # capture both the final result and every intermediate message.
    result = agent.invoke({"messages": [{"role": "user", "content": prompt}]})
    with TRANSCRIPT.open("w") as tf:
        tf.write(json.dumps(result, default=str, indent=2))
    # The last message in the chain is the final answer.
    msgs = result.get("messages", [])
    if msgs:
        last = msgs[-1]
        content = getattr(last, "content", None) or last.get("content", "") if isinstance(last, dict) else last
        print("== final answer ==\n")
        print(content)
        print(f"\n== full state in {TRANSCRIPT.relative_to(ROOT)} ==")
    return 0


if __name__ == "__main__":
    sys.exit(main())
