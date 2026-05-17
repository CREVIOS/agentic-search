"""Native Python usage — no MCP, no agent SDK, no framework.

Same s3:// corpus as the agent examples. Just the `agentic_search`
REST client driving the server like any HTTP service. Works from a
script, a notebook, a FastAPI handler, an Airflow task.

Run:
    bash scripts/rustfs-up.sh
    aws --endpoint-url http://localhost:19000 s3 sync \\
        examples/corpus/data s3://agentic-search-it/corpus
    source scripts/rustfs-env.sh
    target/release/agentic-search serve &
    pip install -e sdks/python/agentic_search
    python examples/native_python_corpus.py
"""

from __future__ import annotations

import os
import sys

CORPUS = os.environ.get("AGENTIC_SEARCH_CORPUS", "s3://agentic-search-it/corpus")
SERVER = os.environ.get("AGENTIC_SEARCH_URL", "http://127.0.0.1:8787")


def main() -> int:
    from agentic_search import Client  # noqa: F401 — used below

    with Client(SERVER) as c:
        if not c.health():
            print(f"server unreachable at {SERVER}", file=sys.stderr)
            return 2

        print(f"== grep {CORPUS!r} for 'graceful shutdown' (top 3, AST widening) ==")
        spans = c.grep(CORPUS, "graceful shutdown", ast=True, max_hits=3)
        for s in spans:
            print(f"  {s.uri}:{s.line_range[0]} [{s.kind}{f' {s.symbol}' if s.symbol else ''}]  {(s.snippet or '')[:80]}")

        print(f"\n== search {CORPUS!r} 'bounded queue backpressure' k=5 ==")
        for s in c.search(CORPUS, "bounded queue backpressure", k=5):
            print(f"  {s.uri}:{s.line_range[0]}  {(s.snippet or '')[:80]}")

        # Read the file behind the top hit, first 400 chars.
        # `Span.uri` is prefix-relative by design — Client.join_uri
        # reassembles it into the full s3:// URI.
        if spans:
            full = Client.join_uri(CORPUS, spans[0].uri)
            print(f"\n== read {full} (first 400 chars) ==")
            txt = c.read(full, offset=0, length=400).text or ""
            print(txt.strip())

    return 0


if __name__ == "__main__":
    sys.exit(main())
