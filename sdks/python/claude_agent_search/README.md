# claude-agent-search

Claude Agent SDK adapter for [agentic-search](../../..).

```python
from claude_agent_sdk import ClaudeAgentOptions, query
from claude_agent_search import as_tools, mcp_server_config

opts = ClaudeAgentOptions(
    mcp_servers=mcp_server_config(),
    allowed_tools=as_tools(),
)
```
