# agenticsearch (Go SDK)

Go client for [agentic-search](https://github.com/CREVIOS/agentic-search) —
the S3-native search runtime for AI agents.

## Install

```bash
go get github.com/CREVIOS/agentic-search/sdks/go/agenticsearch
```

Run the server alongside your process (binds `127.0.0.1:8787` by default):

```bash
agentic-search serve
```

## Usage

```go
package main

import (
    "context"
    "fmt"

    "github.com/CREVIOS/agentic-search/sdks/go/agenticsearch"
)

func main() {
    c := agenticsearch.New("http://127.0.0.1:8787")
    ctx := context.Background()

    spans, err := c.Grep(ctx, "s3://corp/", "HS256", &agenticsearch.GrepOptions{
        AST: true,
    })
    if err != nil {
        panic(err)
    }
    for _, s := range spans {
        fmt.Printf("%s:%d  %s\n", s.URI, s.LineRange[0], s.Snippet)
    }
}
```

## API

- `Grep(ctx, uri, pattern, *GrepOptions)` — ripgrep-as-library; returns `[]Span`.
- `Search(ctx, uri, query, *SearchOptions)` — planner with RRF fusion across
  grep, AST, and (when enabled) the centroid vector index.
- `FindSymbol(ctx, uri, name, *FindSymbolOptions)` — tree-sitter lookup.
- `Ls(ctx, uri, *LsOptions)` / `Read(ctx, uri)` — corpus listing / object fetch.

All methods accept a `context.Context` for cancellation and deadlines.

## License

Apache-2.0
