"""LangChain integration for agentic-search.

Exports a `BaseRetriever` and a `BaseTool` that delegate to a running
agentic-search HTTP server (default `http://127.0.0.1:8787`).

Mirrors the server contract: `uri` is required, the server returns
`spans`, each span has `uri / line_range / kind / symbol / snippet`.
"""

from __future__ import annotations

from typing import Any

from langchain_core.documents import Document
from langchain_core.retrievers import BaseRetriever
from langchain_core.tools import BaseTool


class AgenticSearchRetriever(BaseRetriever):
    """LangChain retriever backed by agentic-search.

    ``uri`` must be supplied (e.g. ``s3://my-corpus/`` or
    ``file:///abs/path``); a retriever is bound to one corpus.
    """

    server_url: str = "http://127.0.0.1:8787"
    uri: str
    k: int = 10

    def _get_relevant_documents(self, query: str, **_: Any) -> list[Document]:
        import requests

        r = requests.post(
            f"{self.server_url}/search",
            json={"uri": self.uri, "query": query, "k": self.k},
            timeout=30,
        )
        r.raise_for_status()
        spans = r.json().get("spans", [])
        return [
            Document(page_content=s.get("snippet") or "", metadata=s)
            for s in spans
        ]


class AgenticSearchTool(BaseTool):
    """LangChain tool wrapping agentic-search's hybrid search endpoint."""

    name: str = "agentic_search"
    description: str = (
        "Hybrid search (parallel ripgrep + tree-sitter AST widening) over an "
        "S3-backed or local corpus. Inputs: uri (e.g. s3://bucket/path/, "
        "file:///abs/path), query (literal text)."
    )
    server_url: str = "http://127.0.0.1:8787"

    def _run(self, uri: str, query: str) -> str:
        import json
        import requests

        r = requests.post(
            f"{self.server_url}/search",
            json={"uri": uri, "query": query, "k": 10},
            timeout=30,
        )
        r.raise_for_status()
        return json.dumps(r.json().get("spans", []))


__all__ = ["AgenticSearchRetriever", "AgenticSearchTool"]
