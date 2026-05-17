# agentic-search MCP server

The agentic-search binary ships an [MCP](https://modelcontextprotocol.io/)
server. Add it to any MCP-aware client (Claude Desktop, Claude Code,
Cursor, Cline, OpenAI Agents SDK with an MCP shim, …) and the host
gets the full agentic-search tool surface for free.

## Stdio

```bash
agentic-search serve --mcp
```

Stdio JSON-RPC 2.0, protocol version `2025-11-25`.

The `claude-agent-search` adapter under `sdks/python/` spawns this
server directly via MCP stdio. The other SDK adapters (OpenAI Agents,
DeepAgents, LangChain, CrewAI, Node, Go) are REST clients that hit
`agentic-search serve` (HTTP) — different transport, identical tool
surface.

## Tools exposed

The schema lock test in `crates/as-server/tests/mcp_schema.rs`
pins this set exactly; renames break CI.

| Tool          | Description                                                       |
| ------------- | ----------------------------------------------------------------- |
| `ls`          | List objects under an `s3://` / `r2://` / `gs://` / `file://` prefix |
| `read`        | Read an object (optional byte range), returns UTF-8 text or bytes |
| `grep`        | ripgrep-as-library over a prefix; spans with line/byte ranges     |
| `find_symbol` | Tree-sitter-verified symbol lookup across case variants           |
| `search`      | Planner: parallel grep + AST widening, RRF-fused. Vector stage    |
|               | runs alongside when an index exists at `.agentic-search/index/…`  |
| `delegate`    | Sub-agent isolation: a search-only subagent loop that compresses  |
|               | citations into a token-frugal answer for the lead agent           |

Every tool ships both an `inputSchema` and an `outputSchema` so MCP
clients can structurally type their responses without parsing free-
form text. Defaults and clamps are documented inline per tool.

## Configuration in Claude Desktop / Claude Code

```json
{
  "mcpServers": {
    "agentic-search": {
      "command": "agentic-search",
      "args": ["serve", "--mcp"]
    }
  }
}
```

For Cursor / Cline / any other MCP host, the same JSON shape works —
they all accept stdio command-and-args server configs.
