"""CrewAI tool wrapper for agentic-search."""

from __future__ import annotations

from crewai.tools import BaseTool


class AgenticSearchTool(BaseTool):
    name: str = "agentic_search"
    description: str = "Hybrid search over the crew's S3-backed corpus + web."
    server_url: str = "http://127.0.0.1:8787"

    def _run(self, query: str) -> str:
        import json
        import requests

        r = requests.post(f"{self.server_url}/search", json={"query": query, "k": 10}, timeout=10)
        r.raise_for_status()
        return json.dumps(r.json().get("hits", []))


__all__ = ["AgenticSearchTool"]
