"""Claude Agent SDK adapter for agentic-search.

Targets ``claude-agent-sdk >= 0.1.81`` (May 2026 shape, with the
``McpStdioServerConfig`` ``type: "stdio"`` field).
"""

from __future__ import annotations

import shutil
from typing import Any


_TOOL_NAMES = (
    "ls",
    "read",
    "grep",
    "find_symbol",
    "search",
)


def as_tools(server_name: str = "agentic_search") -> list[str]:
    """Return MCP tool names exported by the agentic-search MCP server.

    Pass this to ``ClaudeAgentOptions(allowed_tools=...)`` so the SDK
    only surfaces our tools to the model.
    """
    return [f"mcp__{server_name}__{t}" for t in _TOOL_NAMES]


def mcp_server_config(
    binary: str | None = None,
    extra_args: list[str] | None = None,
    env: dict[str, str] | None = None,
    server_name: str = "agentic_search",
) -> dict[str, Any]:
    """Drop-in dict for ``ClaudeAgentOptions(mcp_servers=...)``.

    ``binary`` defaults to ``"agentic-search"`` on PATH. ``extra_args``
    are appended after ``serve --mcp``. ``env`` is passed through to
    the spawned process.
    """
    cmd = binary or shutil.which("agentic-search") or "agentic-search"
    args = ["serve", "--mcp"] + list(extra_args or [])
    cfg: dict[str, Any] = {
        "type": "stdio",
        "command": cmd,
        "args": args,
    }
    if env:
        cfg["env"] = env
    return {server_name: cfg}


__all__ = ["as_tools", "mcp_server_config"]
