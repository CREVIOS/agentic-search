# agentic-search MCP server

The agentic-search binary ships an [MCP](https://modelcontextprotocol.io/)
server. Add it to any MCP-aware client (Claude Desktop, Claude Code, Cursor,
OpenAI Agents SDK, …) and the host gets the full agentic-search tool surface
for free.

## Stdio

```bash
agentic-search serve --mcp
```

## HTTP/SSE (M5+)

```bash
agentic-search serve --mcp-http --bind 0.0.0.0:8788
```

## Tools exposed

| Tool         | Description                                       |
| ------------ | ------------------------------------------------- |
| `ls`         | List objects under an `s3://` / `gs://` / `file://` prefix |
| `glob`       | Glob within a prefix                              |
| `read`       | Read a byte range from an object                  |
| `grep`       | ripgrep-as-library over a prefix                  |
| `search`     | Hybrid (BM25 + vector + web) search               |
| `web_search` | Web-only fallback                                 |
