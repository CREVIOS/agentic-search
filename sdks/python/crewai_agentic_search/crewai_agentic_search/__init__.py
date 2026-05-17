"""CrewAI tool wrapper for agentic-search.

Mirrors the server contract: `uri` is required, the server returns
`spans`. The tool returns the JSON-encoded list of spans.
"""

from __future__ import annotations

from crewai.tools import BaseTool


class AgenticSearchTool(BaseTool):
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


__all__ = ["AgenticSearchTool"]
