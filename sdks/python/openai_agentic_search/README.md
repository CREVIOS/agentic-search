# openai-agentic-search

OpenAI Agents SDK adapter for [agentic-search](../../..).

```python
import asyncio
from agents import Agent, Runner
from openai_agentic_search import all_tools

agent = Agent(
    name="Code finder",
    instructions=(
        "Use only the agentic_search tools to answer. Always pass uri=<corpus root>."
    ),
    tools=all_tools(),
)

async def main():
    result = await Runner.run(
        agent,
        "Find the JWT verification function in s3://my-corpus/",
    )
    print(result.final_output)

asyncio.run(main())
```

The HTTP server URL defaults to `http://127.0.0.1:8787` and can be
overridden with `AGENTIC_SEARCH_URL`.
