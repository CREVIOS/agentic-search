"""End-to-end example: agentic-search exposed to the Claude Agent SDK over MCP.

Prereqs:
    pip install claude-agent-sdk agentic-search
    agentic-search serve --mcp &  # or let the SDK spawn it

Run:
    python examples/claude_agent_sdk_example.py
"""

import asyncio

from claude_agent_sdk import ClaudeAgentOptions, query


async def main() -> None:
    options = ClaudeAgentOptions(
        mcp_servers={
            "agentic_search": {
                "command": "agentic-search",
                "args": ["serve", "--mcp"],
            }
        },
        # Restrict the agent to the agentic-search tools so it does not wander.
        allowed_tools=[
            "mcp__agentic_search__ls",
            "mcp__agentic_search__grep",
            "mcp__agentic_search__search",
        ],
    )

    prompt = (
        "Find every place in s3://my-corpus/ that still references the legacy "
        "HS256 JWT setting and summarize them."
    )

    async for msg in query(prompt=prompt, options=options):
        print(msg)


if __name__ == "__main__":
    asyncio.run(main())
