"""Claude Agent SDK + agentic-search MCP — real corpus, complex multi-step query.

Spawns the local `agentic-search` binary over MCP stdio, points it at
the 4 MB markdown corpus built by `examples/corpus/build.sh` (Rust Book
+ Kubernetes concepts + Tokio tutorial), then asks Claude a question
that forces it to use `grep`, `find_symbol`, and `read` tools together.

Run:
    bash examples/corpus/build.sh
    cargo build --release -p agentic-search-cli
    source .venv-examples/bin/activate
    python examples/claude_agent_corpus.py
"""

from __future__ import annotations

import asyncio
import json
import os
import pathlib
import sys
from datetime import datetime

ROOT = pathlib.Path(__file__).resolve().parent.parent
BIN = ROOT / "target" / "release" / "agentic-search"
CORPUS = ROOT / "examples" / "corpus" / "data"
TRANSCRIPT = ROOT / "examples" / "transcripts" / f"claude_{datetime.utcnow().strftime('%Y%m%dT%H%M%S')}.jsonl"


def load_env() -> None:
    env_path = ROOT / ".env"
    if not env_path.exists():
        return
    for line in env_path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        k, _, v = line.partition("=")
        os.environ.setdefault(k.strip(), v.strip())
    # The SDK reads ANTHROPIC_API_KEY; the user's .env named it
    # APP_CLAUDE_API_KEY, so mirror it across.
    if not os.environ.get("ANTHROPIC_API_KEY") and os.environ.get("APP_CLAUDE_API_KEY"):
        os.environ["ANTHROPIC_API_KEY"] = os.environ["APP_CLAUDE_API_KEY"]


async def main() -> int:
    load_env()
    if not BIN.exists():
        print(f"binary missing: {BIN}\n  run: cargo build --release -p agentic-search-cli", file=sys.stderr)
        return 2
    if not CORPUS.exists():
        print(f"corpus missing: {CORPUS}\n  run: bash examples/corpus/build.sh", file=sys.stderr)
        return 2
    if not os.environ.get("ANTHROPIC_API_KEY"):
        print("ANTHROPIC_API_KEY missing (or APP_CLAUDE_API_KEY in .env)", file=sys.stderr)
        return 2

    from claude_agent_sdk import ClaudeAgentOptions, query

    # Pass through the AWS env that scripts/rustfs-env.sh exports so
    # the spawned server can sign S3 requests at the RustFS endpoint.
    server_env = {
        k: v
        for k, v in os.environ.items()
        if k.startswith("AWS_") or k in ("RUSTFS_S3_TEST",)
    }
    server = {
        "type": "stdio",
        "command": str(BIN),
        "args": ["serve", "--mcp"],
        "env": server_env,
    }
    tools = [
        "mcp__agentic_search__ls",
        "mcp__agentic_search__read",
        "mcp__agentic_search__grep",
        "mcp__agentic_search__find_symbol",
        "mcp__agentic_search__search",
    ]
    # The corpus URI can be a local path or an S3-shaped URI. The
    # examples/README ships the RustFS-backed s3:// setup; falls back
    # to file:// when the env var isn't set.
    corpus_uri = os.environ.get(
        "AGENTIC_SEARCH_CORPUS", f"file://{CORPUS}"
    )
    opts = ClaudeAgentOptions(
        mcp_servers={"agentic_search": server},
        allowed_tools=tools,
        system_prompt=(
            f"You are answering questions about a 10,843-file markdown "
            f"corpus at {corpus_uri} covering: rust-lang/book (Rust "
            "stdlib), tokio-rs/website (Tokio async runtime), "
            "kubernetes/website (k8s concepts), mdn/content "
            "(web/javascript, web/api, web/css). Always use the "
            "agentic_search MCP tools (grep, find_symbol, read) to "
            "ground every claim — never guess from training data. "
            "When citing, include the file path. Be concise."
        ),
        max_turns=25,
    )

    prompt = (
        "Across the corpus, find ONE concrete example each of how the "
        "Rust async ecosystem, the Web Platform (MDN), and the "
        "Kubernetes API model the same idea of *bounded queues with "
        "backpressure*. For each, cite an exact filename and one "
        "short quoted passage that shows the bound or backpressure "
        "mechanism. End with a two-sentence synthesis of what is "
        "common across all three."
    )

    print(f"== prompt ==\n{prompt}\n")
    print(f"== transcript → {TRANSCRIPT.relative_to(ROOT)} ==\n")
    TRANSCRIPT.parent.mkdir(parents=True, exist_ok=True)

    final_text: list[str] = []
    with TRANSCRIPT.open("w") as tf:
        async for msg in query(prompt=prompt, options=opts):
            kind = type(msg).__name__
            tf.write(json.dumps({"kind": kind, "repr": repr(msg)}) + "\n")
            tf.flush()
            # The SDK exposes block types as named classes
            # (TextBlock, ToolUseBlock, ToolResultBlock). Look at the
            # class name rather than a dict shape.
            blocks = getattr(msg, "content", None)
            if isinstance(blocks, list):
                for block in blocks:
                    btype = type(block).__name__
                    if btype == "TextBlock":
                        chunk = getattr(block, "text", "")
                        sys.stdout.write(chunk)
                        sys.stdout.flush()
                        final_text.append(chunk)
                    elif btype == "ToolUseBlock":
                        name = getattr(block, "name", "?")
                        args = getattr(block, "input", {})
                        sys.stdout.write(
                            f"\n  [tool→] {name}({json.dumps(args, default=str)[:140]})\n"
                        )
                        sys.stdout.flush()
                    elif btype == "ToolResultBlock":
                        # Brief — full result lives in the transcript.
                        sys.stdout.write(f"  [tool← {btype}]\n")
                        sys.stdout.flush()
        sys.stdout.write("\n")

    print(f"\n== done — full transcript in {TRANSCRIPT.relative_to(ROOT)} ==")
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
