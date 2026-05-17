module agentic-search-go-demo

go 1.22

// Use the in-tree Go SDK during demo runs. Switch to the published
// module path once `agenticsearch` is on pkg.go.dev.
replace github.com/CREVIOS/agentic-search/sdks/go/agenticsearch => ../../sdks/go/agenticsearch

require github.com/CREVIOS/agentic-search/sdks/go/agenticsearch v0.0.0-00010101000000-000000000000
