"""OpenAI Agents SDK adapter for agentic-search.

Targets ``openai-agents >= 0.17``. Each tool is a ``@function_tool``
that hits the local agentic-search HTTP server (default
``http://127.0.0.1:8787``). All tools require ``uri`` so the agent
picks which corpus to search; ``query`` / ``pattern`` / ``symbol``
carry the search intent.

Quickstart::

    from agents import Agent, Runner
    from openai_agentic_search import all_tools

    agent = Agent(
        name="Code finder",
        instructions=(
            "Use only the agentic_search tools to answer. "
            "Always pass uri=<corpus root> and a focused query."
        ),
        tools=all_tools(),
    )
    result = await Runner.run(agent, "Find the JWT verification function in s3://my-corpus/")
"""

from __future__ import annotations

from typing import Any


def _server_url() -> str:
    import os

    return os.environ.get("AGENTIC_SEARCH_URL", "http://127.0.0.1:8787")


def _post(path: str, body: dict[str, Any]) -> dict[str, Any]:
    import requests

    r = requests.post(f"{_server_url()}{path}", json=body, timeout=30)
    if not r.ok:
        raise RuntimeError(f"agentic-search {path}: {r.status_code} {r.text}")
    return r.json()


def make_tools():
    """Return the set of ``@function_tool``-decorated callables.

    The decorator is applied lazily so ``openai-agents`` is only imported
    when the caller actually uses the adapter.
    """
    from agents import function_tool

    @function_tool
    def agentic_search(uri: str, query: str, k: int = 10) -> list[dict[str, Any]]:
        """Hybrid search (parallel ripgrep + tree-sitter span widening +
        optional centroid vector stage) over an S3-backed or local corpus.

        Args:
            uri:   e.g. ``s3://my-corpus/`` or ``file:///abs/path``.
            query: natural-language or literal query text.
            k:     max number of spans to return.
        """
        return _post("/search", {"uri": uri, "query": query, "k": k}).get("spans", [])

    @function_tool
    def agentic_grep(
        uri: str,
        pattern: str,
        case_insensitive: bool = False,
        ast: bool = True,
        max_hits: int = 200,
    ) -> list[dict[str, Any]]:
        """Parallel ripgrep over a prefix, with optional AST span widening."""
        return _post(
            "/grep",
            {
                "uri": uri,
                "pattern": pattern,
                "case_insensitive": case_insensitive,
                "ast": ast,
                "max_hits": max_hits,
                "concurrency": 32,
            },
        ).get("spans", [])

    @function_tool
    def agentic_find_symbol(
        uri: str, symbol: str, max_hits: int = 20
    ) -> list[dict[str, Any]]:
        """Locate a function / class / method by exact name across a prefix.
        Returns AST-widened spans only when tree-sitter confirms the name."""
        return _post(
            "/find",
            {"uri": uri, "symbol": symbol, "max_hits": max_hits},
        ).get("spans", [])

    @function_tool
    def agentic_read(
        uri: str, offset: int | None = None, length: int | None = None
    ) -> dict[str, Any]:
        """Read an object's bytes (optional byte range). Returns text when UTF-8."""
        body: dict[str, Any] = {"uri": uri}
        if offset is not None:
            body["offset"] = offset
        if length is not None:
            body["length"] = length
        return _post("/read", body)

    @function_tool
    def agentic_ls(
        uri: str, glob: str | None = None, limit: int = 200
    ) -> list[dict[str, Any]]:
        """List objects under a URI prefix (optionally glob-filtered)."""
        body: dict[str, Any] = {"uri": uri, "limit": limit}
        if glob:
            body["glob"] = glob
        return _post("/ls", body).get("entries", [])

    return [
        agentic_search,
        agentic_grep,
        agentic_find_symbol,
        agentic_read,
        agentic_ls,
    ]


def all_tools():
    """Alias for `make_tools()`; matches the deepagents adapter naming."""
    return make_tools()


__all__ = ["make_tools", "all_tools"]
