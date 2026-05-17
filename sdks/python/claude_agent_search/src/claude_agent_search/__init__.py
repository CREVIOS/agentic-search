"""Claude Agent SDK adapter for agentic-search.

Exposes ready-to-use tools that an agent built on `claude-agent-sdk` can call
without any glue code.
"""

from __future__ import annotations

from typing import Any


def as_tools() -> list[str]:
    """Return the MCP tool names exported by the agentic-search MCP server."""
    return [
        "mcp__agentic_search__ls",
        "mcp__agentic_search__glob",
        "mcp__agentic_search__read",
        "mcp__agentic_search__grep",
        "mcp__agentic_search__search",
        "mcp__agentic_search__web_search",
    ]


def mcp_server_config(binary: str = "agentic-search") -> dict[str, Any]:
    """Drop-in MCP server config for `ClaudeAgentOptions.mcp_servers`."""
    return {
        "agentic_search": {
            "command": binary,
            "args": ["serve", "--mcp"],
        }
    }


__all__ = ["as_tools", "mcp_server_config"]
