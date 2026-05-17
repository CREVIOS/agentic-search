// Go demo: drive the agentic-search REST server with the Go SDK
// against the real S3 corpus on RustFS. Mirrors the Node example
// in examples/node_corpus.ts. Build:
//
//   cd examples/go_corpus
//   go mod init agentic-search-demo  # one-time
//   go mod tidy
//   go run .
package main

import (
	"context"
	"fmt"
	"net/http"
	"os"
	"time"

	"github.com/CREVIOS/agentic-search/sdks/go/agenticsearch"
)

func main() {
	serverURL := envOr("AGENTIC_SEARCH_URL", "http://127.0.0.1:8787")
	corpus := envOr("AGENTIC_SEARCH_CORPUS", "s3://agentic-search-it/corpus")

	fmt.Printf("== /health (%s) ==\n", serverURL)
	resp, err := http.Get(serverURL + "/health")
	if err != nil {
		fail("health", err)
	}
	resp.Body.Close()
	fmt.Printf("  %s\n", resp.Status)

	c := agenticsearch.New(serverURL,
		agenticsearch.WithHTTPClient(&http.Client{Timeout: 30 * time.Second}))
	ctx := context.Background()

	fmt.Printf("\n== /grep %s for \"graceful shutdown\" (top 5) ==\n", corpus)
	spans, err := c.Grep(ctx, corpus, "graceful shutdown", &agenticsearch.GrepOptions{MaxHits: 5})
	if err != nil {
		fail("grep", err)
	}
	for _, s := range spans {
		fmt.Printf("  %s:%d  %s\n", s.URI, s.LineRange[0], snip(s.Snippet, 80))
	}

	fmt.Printf("\n== /find_symbol %s symbol \"verify_jwt\" (max 3) ==\n", corpus)
	syms, err := c.FindSymbol(ctx, corpus, "verify_jwt", &agenticsearch.FindSymbolOptions{MaxHits: 3})
	if err != nil {
		fail("find_symbol", err)
	}
	fmt.Printf("  %d hits (expected 0 — corpus is markdown, no code symbols)\n", len(syms))

	fmt.Printf("\n== /search %s \"backpressure unbounded queue\" k=3 ==\n", corpus)
	hits, err := c.Search(ctx, corpus, "backpressure unbounded queue", &agenticsearch.SearchOptions{K: 3})
	if err != nil {
		fail("search", err)
	}
	for _, s := range hits {
		fmt.Printf("  %s:%d  %s\n", s.URI, s.LineRange[0], snip(s.Snippet, 80))
	}
}

func envOr(k, d string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return d
}

func snip(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n]
}

func fail(stage string, err error) {
	fmt.Fprintf(os.Stderr, "%s: %v\n", stage, err)
	os.Exit(1)
}
