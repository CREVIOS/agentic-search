# agentic-search (Python)

Native Python client for the [agentic-search](https://github.com/CREVIOS/agentic-search)
REST server. **No MCP. No agent framework dependency.** Just a thin
HTTP client over `agentic-search serve`.

```bash
pip install agentic-search
```

## Usage

```python
from agentic_search import Client

c = Client("http://127.0.0.1:8787")

# grep across an S3 corpus
hits = c.grep("s3://my-corpus/", "TODO(security)", ast=True)

# hybrid search (grep + AST + optional centroid vector)
spans = c.search("s3://my-corpus/", "graceful shutdown", k=10)

# read the file behind a hit
body = c.read(spans[0].uri).text
```

Works the same against `s3://`, `r2://`, `gs://`, or `file:///abs/path`
— the server is the only thing that needs S3 credentials.

For framework-specific wrappers (Claude Agent SDK / DeepAgents /
LangChain / CrewAI / OpenAI Agents), use the per-framework adapters
under `sdks/python/*_agentic_search/`. This package is the **native,
no-framework** path.

## License

Apache-2.0
