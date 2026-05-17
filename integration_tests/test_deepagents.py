"""Drive LangChain DeepAgents against the agentic-search HTTP server.

Runs locally with ANTHROPIC_API_KEY in env (loaded from .env). Brings
up the agentic-search HTTP server on :8787, hands it to a
``create_deep_agent`` with our search tools, and asserts the agent
actually called at least one of the search endpoints.

Usage::

    cargo build --release -p as-cli
    /tmp/asv/bin/python -m pip install deepagents langchain-anthropic
    /tmp/asv/bin/python integration_tests/test_deepagents.py
"""

from __future__ import annotations

import os
import shutil
import socket
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target" / "release" / "agentic-search"
CORPUS = Path("/tmp/agentic-search-deepagents")


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


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def wait_for(url: str, timeout: float = 10.0) -> None:
    import urllib.request

    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            with urllib.request.urlopen(url, timeout=1.0) as r:
                if r.status < 500:
                    return
        except Exception:
            pass
        time.sleep(0.2)
    raise TimeoutError(f"server did not come up at {url}")


def make_tools(server_url: str) -> list[Any]:
    """Return DeepAgents-friendly callable tools backed by HTTP."""
    import requests
    from langchain_core.tools import tool

    @tool
    def agentic_grep(
        uri: str,
        pattern: str,
        ast: bool = True,
        max_hits: int = 50,
    ) -> str:
        """Parallel ripgrep over an S3/local URI, with optional AST span widening.

        Args:
            uri: e.g. "s3://my-corpus/" or "file:///tmp/...". Required.
            pattern: regex pattern (literal text gets matched too).
            ast: widen each hit to its enclosing function/class/method.
            max_hits: cap on total spans returned.
        """
        r = requests.post(
            f"{server_url}/grep",
            json={
                "uri": uri,
                "pattern": pattern,
                "ast": ast,
                "max_hits": max_hits,
                "concurrency": 32,
            },
            timeout=30,
        )
        r.raise_for_status()
        spans = r.json().get("spans", [])
        return "\n".join(
            f"{s['uri']}:{s['line_range'][0]}-{s['line_range'][1]} "
            f"[{s['kind']} {s.get('symbol','')}] {s.get('snippet','')[:120]}"
            for s in spans
        ) or "(no matches)"

    @tool
    def agentic_find_symbol(uri: str, symbol: str, max_hits: int = 20) -> str:
        """Locate a function/class/method by exact name across a prefix."""
        r = requests.post(
            f"{server_url}/find",
            json={"uri": uri, "symbol": symbol, "max_hits": max_hits},
            timeout=30,
        )
        r.raise_for_status()
        spans = r.json().get("spans", [])
        return "\n".join(
            f"{s['uri']}:{s['line_range'][0]}-{s['line_range'][1]} "
            f"[{s['kind']} {s.get('symbol','')}] {s.get('snippet','')[:120]}"
            for s in spans
        ) or "(no symbol matches)"

    return [agentic_grep, agentic_find_symbol]


def main() -> int:
    load_env()
    if "ANTHROPIC_API_KEY" not in os.environ:
        print("ANTHROPIC_API_KEY missing; aborting", file=sys.stderr)
        return 2
    if not BIN.exists():
        print(f"binary missing: {BIN}; cargo build --release -p as-cli", file=sys.stderr)
        return 2
    try:
        from deepagents import create_deep_agent  # type: ignore[import-not-found]
    except ImportError:
        print("pip install deepagents langchain-anthropic", file=sys.stderr)
        return 2

    build_corpus()

    port = free_port()
    bind = f"127.0.0.1:{port}"
    server_url = f"http://{bind}"
    server = subprocess.Popen(
        [str(BIN), "serve", "--bind", bind],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        wait_for(f"{server_url}/health", timeout=10)
        tools = make_tools(server_url)
        agent = create_deep_agent(
            model="anthropic:claude-haiku-4-5-20251001",
            tools=tools,
            system_prompt=(
                "You are a code-search assistant. Use ONLY the provided "
                "agentic_grep and agentic_find_symbol tools. Never invent "
                "answers without calling a tool."
            ),
        )
        prompt = (
            "Use agentic_find_symbol with symbol=\"beta\" against the URI "
            f"file://{CORPUS}. Report the result as JSON: "
            '{"file": <relative path>, "line_range": [start, end], '
            '"symbol": <name>}.'
        )
        result = agent.invoke({"messages": [{"role": "user", "content": prompt}]})
        # The final message contains the model's answer
        messages = result.get("messages", []) if isinstance(result, dict) else []
        tool_invocations = [
            m for m in messages
            if getattr(m, "type", "") == "tool" or "tool_calls" in getattr(m, "additional_kwargs", {})
        ]
        text = ""
        for m in messages:
            content = getattr(m, "content", None)
            if isinstance(content, str) and content.strip():
                text = content
        print("=== final ===")
        print(text)
        print(f"messages: {len(messages)}, tool invocations: {len(tool_invocations)}")
        # Must have actually invoked at least one tool.
        called = any(
            (getattr(m, "name", "") in ("agentic_grep", "agentic_find_symbol"))
            or "agentic_" in repr(m)
            for m in messages
        )
        if not called:
            print("FAIL: no agentic_search tool invoked", file=sys.stderr)
            return 1
        print("OK")
        return 0
    finally:
        server.terminate()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()


if __name__ == "__main__":
    sys.exit(main())
