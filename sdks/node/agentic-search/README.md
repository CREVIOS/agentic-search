# @agentic-search/sdk

Node / TypeScript client for [agentic-search](https://github.com/CREVIOS/agentic-search) —
the S3-native search runtime for AI agents.

## Install

```bash
pnpm add @agentic-search/sdk
# or: npm i @agentic-search/sdk
```

Run the server in a separate process (binds `127.0.0.1:8787` by default):

```bash
agentic-search serve
```

## Usage

```ts
import { AgenticSearchClient } from "@agentic-search/sdk";

const client = new AgenticSearchClient("http://127.0.0.1:8787");

// grep across an S3 corpus
const hits = await client.grep("s3://corp/", "HS256", { ast: true });

// hybrid search (grep + AST + vector + RRF fusion)
const result = await client.search("s3://corp/", "verify jwt token", { k: 10 });

// list / read / find_symbol
await client.ls("s3://corp/docs/");
await client.read("s3://corp/docs/README.md");
await client.findSymbol("s3://corp/src/", "verify_jwt");
```

All requests pass the agent's working corpus URI on every call — same shape
as the underlying REST contract. The client carries no implicit state.

## API

- `grep(uri, pattern, opts?)` — ripgrep-as-library; returns `spans`.
- `search(uri, query, opts?)` — planner with RRF fusion across grep, AST,
  and (when enabled) the centroid vector index.
- `ls(uri, opts?)` / `read(uri, opts?)` / `findSymbol(uri, name, opts?)`.

See the [agentic-search README](https://github.com/CREVIOS/agentic-search#readme)
for the full tool surface and architecture.

## License

Apache-2.0
