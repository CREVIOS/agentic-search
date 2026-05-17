"""Drive the Claude Agent SDK with the agentic-search MCP server.

Runs locally only — needs ``ANTHROPIC_API_KEY`` in the environment
(loaded from .env). The test asks Claude to find a TODO inside a
specific Python function across a local prefix using the MCP-exposed
tools, and asserts that it called ``find_symbol`` or ``grep`` (not just
``read``) to do so.

Usage::

    cd /Users/asif/Desktop/opensource
    cargo build --release -p as-cli
    pip install claude-agent-sdk python-dotenv
    python integration_tests/test_claude_agent_sdk.py
"""

from __future__ import annotations

import asyncio
import os
import shutil
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target" / "release" / "agentic-search"
CORPUS = Path("/tmp/agentic-search-itest")


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


def build_corpus() -> None:
    if CORPUS.exists():
        shutil.rmtree(CORPUS)
    CORPUS.mkdir(parents=True)
    (CORPUS / "a.py").write_text(
        "def alpha(x):\n    return x + 1\n\n"
        "def beta(x):\n    # TODO: optimize beta\n    return x * 2\n"
    )
    (CORPUS / "b.rs").write_text(
        "fn one() -> i32 { 1 }\n\n"
        "fn two() -> i32 {\n    // TODO: rewrite two\n    2\n}\n"
    )


async def main() -> int:
    load_env()
    if "ANTHROPIC_API_KEY" not in os.environ:
        print("ANTHROPIC_API_KEY missing; aborting", file=sys.stderr)
        return 2
    if not BIN.exists():
        print(f"agentic-search binary missing at {BIN}; run cargo build --release -p as-cli", file=sys.stderr)
        return 2

    build_corpus()

    try:
        from claude_agent_sdk import ClaudeAgentOptions, query  # type: ignore[import-not-found]
    except ImportError:
        print("pip install claude-agent-sdk first", file=sys.stderr)
        return 2

    prompt = (
        f"Use only the agentic_search tools to find the TODO comment inside the "
        f"`beta` function in the corpus rooted at file://{CORPUS}. "
        "Call find_symbol with symbol=\"beta\" first, then report the line range "
        "and the file path. Output JSON with keys: file, line_range, symbol."
    )

    options = ClaudeAgentOptions(
        mcp_servers={
            "agentic_search": {
                "type": "stdio",
                "command": str(BIN),
                "args": ["serve", "--mcp"],
            }
        },
        allowed_tools=[
            "mcp__agentic_search__ls",
            "mcp__agentic_search__read",
            "mcp__agentic_search__grep",
            "mcp__agentic_search__find_symbol",
            "mcp__agentic_search__search",
        ],
        max_turns=8,
    )

    tool_calls: list[str] = []
    async for msg in query(prompt=prompt, options=options):
        # The SDK yields typed messages; we only care about which tools fire.
        text = repr(msg)
        for t in ("find_symbol", "grep", "ls", "read", "search"):
            tag = f"agentic_search__{t}"
            if tag in text and t not in tool_calls:
                tool_calls.append(t)
        print(text)

    print("\n=== tool calls observed ===")
    print(tool_calls)
    if not any(t in tool_calls for t in ("find_symbol", "grep", "search")):
        print("FAIL: none of find_symbol/grep/search were called", file=sys.stderr)
        return 1
    print("OK")
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
