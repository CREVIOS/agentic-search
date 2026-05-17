"""DeepAgents adapter for agentic-search."""

from __future__ import annotations

from typing import Any, Callable


def search_tool(server_url: str = "http://127.0.0.1:8787") -> Callable[..., Any]:
    """Return a DeepAgents-compatible tool callable that hits a running
    agentic-search HTTP server."""

    import requests

    def search(query: str, k: int = 10) -> list[dict[str, Any]]:
        """Hybrid search over the agent's S3-backed corpus."""
        r = requests.post(f"{server_url}/search", json={"query": query, "k": k}, timeout=10)
        r.raise_for_status()
        return r.json().get("hits", [])

    search.__name__ = "agentic_search"
    return search


__all__ = ["search_tool"]
