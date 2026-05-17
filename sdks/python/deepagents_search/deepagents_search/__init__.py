"""DeepAgents adapter for agentic-search.

Targets ``deepagents >= 0.6.1``. Provides a callable tool that hits the
agentic-search HTTP server (started with ``agentic-search serve``)
"""

from __future__ import annotations

from typing import Any


def search_tool(server_url: str = "http://127.0.0.1:8787"):
    """Return a DeepAgents-compatible tool callable.

    The tool calls the agentic-search HTTP ``/search`` endpoint, which
    runs the planner (grep + AST widening) over the given URI.
    """

    import requests

    def agentic_search(uri: str, query: str, k: int = 20) -> list[dict[str, Any]]:
        """Hybrid search over an S3-backed (or local) corpus.

        Args:
            uri:   The corpus root, e.g. ``s3://my-corpus/`` or
                   ``file:///abs/path``.
            query: The search query (literal text; gets regex-escaped).
            k:     Max number of spans to return.
        """
        resp = requests.post(
            f"{server_url}/search",
            json={"uri": uri, "query": query, "k": k},
            timeout=30,
        )
        resp.raise_for_status()
        return resp.json().get("spans", [])

    return agentic_search


def grep_tool(server_url: str = "http://127.0.0.1:8787"):
    """ripgrep over a prefix, with optional AST span widening."""
    import requests

    def agentic_grep(
        uri: str,
        pattern: str,
        case_insensitive: bool = False,
        ast: bool = True,
        max_hits: int = 200,
    ) -> list[dict[str, Any]]:
        resp = requests.post(
            f"{server_url}/grep",
            json={
                "uri": uri,
                "pattern": pattern,
                "case_insensitive": case_insensitive,
                "ast": ast,
                "max_hits": max_hits,
            },
            timeout=30,
        )
        resp.raise_for_status()
        return resp.json().get("spans", [])

    return agentic_grep


def find_symbol_tool(server_url: str = "http://127.0.0.1:8787"):
    """Locate a function/class/method across a prefix by exact name."""
    import requests

    def agentic_find_symbol(uri: str, symbol: str, max_hits: int = 200) -> list[dict[str, Any]]:
        resp = requests.post(
            f"{server_url}/find",
            json={"uri": uri, "symbol": symbol, "max_hits": max_hits},
            timeout=30,
        )
        resp.raise_for_status()
        return resp.json().get("spans", [])

    return agentic_find_symbol


def all_tools(server_url: str = "http://127.0.0.1:8787") -> list[Any]:
    """Return the full agentic-search tool set for ``deepagents``."""
    return [search_tool(server_url), grep_tool(server_url), find_symbol_tool(server_url)]


__all__ = ["search_tool", "grep_tool", "find_symbol_tool", "all_tools"]
