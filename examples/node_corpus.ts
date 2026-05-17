// Node / TypeScript demo: drive the agentic-search REST server with
// the @agentic-search/sdk client against the real S3 corpus on
// RustFS. The server signs SigV4 requests at
// http://localhost:19000; the client just speaks HTTP+JSON to the
// agentic-search server.
//
// Run:
//   bash scripts/rustfs-up.sh
//   aws --endpoint-url http://localhost:19000 s3 sync \
//       examples/corpus/data s3://agentic-search-it/corpus
//   source scripts/rustfs-env.sh
//   target/release/agentic-search serve &
//   pnpm install
//   pnpm tsx examples/node_corpus.ts
//
// Output mirrors what an agent would receive from the
// `agentic_search` tool.

import { AgenticSearchClient } from "../sdks/node/agentic-search/src/index.js";

const SERVER_URL = process.env.AGENTIC_SEARCH_URL ?? "http://127.0.0.1:8787";
const CORPUS = process.env.AGENTIC_SEARCH_CORPUS ?? "s3://agentic-search-it/corpus";

async function main() {
  const c = new AgenticSearchClient(SERVER_URL);

  console.log(`== /health (${SERVER_URL}) ==`);
  // Health is not on the SDK surface; do a raw fetch to confirm.
  const h = await fetch(`${SERVER_URL}/health`);
  console.log(`  ${h.status} ${h.statusText}`);

  console.log(`\n== /grep ${CORPUS} for "graceful shutdown" (top 5) ==`);
  const spans = await c.grep(CORPUS, "graceful shutdown", { ast: false, max_hits: 5 });
  for (const s of spans) {
    console.log(`  ${s.uri}:${s.line_range[0]}  ${(s.snippet ?? "").slice(0, 80)}`);
  }

  console.log(`\n== /find_symbol ${CORPUS} symbol "verify_jwt" (max 3) ==`);
  const syms = await c.findSymbol(CORPUS, "verify_jwt", { max_hits: 3 });
  console.log(`  ${syms.length} hits (expected 0 — corpus is markdown, no code symbols)`);

  console.log(`\n== /search ${CORPUS} "backpressure unbounded queue" k=3 ==`);
  const hits = await c.search(CORPUS, "backpressure unbounded queue", { k: 3 });
  for (const s of hits) {
    console.log(`  ${s.uri}:${s.line_range[0]}  ${(s.snippet ?? "").slice(0, 80)}`);
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
