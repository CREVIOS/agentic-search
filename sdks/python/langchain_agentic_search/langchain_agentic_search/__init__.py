"""LangChain integration for agentic-search.

Exports a `BaseRetriever` and a `BaseTool` that delegate to a running
agentic-search HTTP server (default `http://127.0.0.1:8787`).
"""

from __future__ import annotations

from typing import Any

from langchain_core.documents import Document
from langchain_core.retrievers import BaseRetriever
from langchain_core.tools import BaseTool


class AgenticSearchRetriever(BaseRetriever):
    """LangChain retriever backed by agentic-search."""

    server_url: str = "http://127.0.0.1:8787"
    k: int = 10

    def _get_relevant_documents(self, query: str, **_: Any) -> list[Document]:
        import requests

        r = requests.post(
            f"{self.server_url}/search", json={"query": query, "k": self.k}, timeout=10
        )
        r.raise_for_status()
        return [
            Document(page_content=h.get("snippet") or "", metadata=h)
            for h in r.json().get("hits", [])
        ]


class AgenticSearchTool(BaseTool):
    """LangChain tool wrapping agentic-search's hybrid search endpoint."""

    name: str = "agentic_search"
    description: str = "Hybrid search over the agent's S3-backed corpus + web."
    server_url: str = "http://127.0.0.1:8787"

    def _run(self, query: str) -> str:
        import json
        import requests

        r = requests.post(f"{self.server_url}/search", json={"query": query, "k": 10}, timeout=10)
        r.raise_for_status()
        return json.dumps(r.json().get("hits", []))


__all__ = ["AgenticSearchRetriever", "AgenticSearchTool"]
