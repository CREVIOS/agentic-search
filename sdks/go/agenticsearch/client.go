// Package agenticsearch is the Go client for the agentic-search REST server.
//
// Spin up the server alongside your agent process:
//
//	agentic-search serve
//
// Then:
//
//	c := agenticsearch.New("http://127.0.0.1:8787")
//	hits, err := c.Grep(ctx, "s3://corp/", "HS256", nil)
//
// The client mirrors the server's REST contract verbatim: `uri` is
// required on every call, responses come back as `spans`.
package agenticsearch

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"
)

// Client is a thin HTTP client around the agentic-search REST server.
// Construct via New. Safe for concurrent use.
type Client struct {
	baseURL string
	http    *http.Client
}

// Option configures a Client at construction time.
type Option func(*Client)

// WithHTTPClient lets callers supply a pre-tuned *http.Client (e.g. with
// custom transport, proxy, timeouts, or TLS config).
func WithHTTPClient(h *http.Client) Option {
	return func(c *Client) { c.http = h }
}

// New returns a Client targeting baseURL (e.g. "http://127.0.0.1:8787").
// Default HTTP timeout is 30s; override with WithHTTPClient.
func New(baseURL string, opts ...Option) *Client {
	c := &Client{
		baseURL: baseURL,
		http:    &http.Client{Timeout: 30 * time.Second},
	}
	for _, o := range opts {
		o(c)
	}
	return c
}

// SpanKind enumerates the AST/lexical kinds returned by the server.
type SpanKind string

const (
	SpanLine     SpanKind = "line"
	SpanBlock    SpanKind = "block"
	SpanFunction SpanKind = "function"
	SpanMethod   SpanKind = "method"
	SpanClass    SpanKind = "class"
	SpanModule   SpanKind = "module"
)

// ByteRange is the [start, end) byte offset of a span in its source file.
type ByteRange struct {
	Start uint64 `json:"start"`
	End   uint64 `json:"end"`
}

// Span is the unit of return for every search-style endpoint.
type Span struct {
	URI       string    `json:"uri"`
	ByteRange ByteRange `json:"byte_range"`
	LineRange [2]uint32 `json:"line_range"`
	Symbol    string    `json:"symbol,omitempty"`
	Kind      SpanKind  `json:"kind"`
	Snippet   string    `json:"snippet,omitempty"`
	Score     float64   `json:"score"`
}

// GrepOptions controls /grep behavior.
type GrepOptions struct {
	CaseInsensitive bool  `json:"case_insensitive,omitempty"`
	MaxHits         int   `json:"max_hits,omitempty"`
	Concurrency     int   `json:"concurrency,omitempty"`
	AST             bool  `json:"ast,omitempty"`
}

// SearchOptions controls /search behavior.
type SearchOptions struct {
	K int `json:"k,omitempty"`
}

// FindSymbolOptions controls /find_symbol behavior.
type FindSymbolOptions struct {
	MaxHits     int `json:"max_hits,omitempty"`
	Concurrency int `json:"concurrency,omitempty"`
}

// LsOptions controls /ls behavior.
type LsOptions struct {
	Glob  string `json:"glob,omitempty"`
	Limit int    `json:"limit,omitempty"`
}

// SpansResponse is the JSON envelope returned by every span-emitting
// endpoint.
type SpansResponse struct {
	Spans []Span `json:"spans"`
}

// Entry is one listing row returned by /ls.
type Entry struct {
	Key          string `json:"key"`
	Size         uint64 `json:"size"`
	LastModified int64  `json:"last_modified,omitempty"`
}

// ListResponse is the JSON envelope returned by /ls.
type ListResponse struct {
	Entries []Entry `json:"entries"`
}

// ReadResponse is the JSON envelope returned by /read.
type ReadResponse struct {
	URI     string `json:"uri"`
	Content string `json:"content"`
	Bytes   uint64 `json:"bytes"`
}

// Grep issues a /grep request. `pattern` is a regex; `opts` may be nil.
func (c *Client) Grep(ctx context.Context, uri, pattern string, opts *GrepOptions) ([]Span, error) {
	body := map[string]any{"uri": uri, "pattern": pattern}
	mergeOptions(body, opts)
	var out SpansResponse
	if err := c.post(ctx, "/grep", body, &out); err != nil {
		return nil, err
	}
	return out.Spans, nil
}

// Search issues a /search request: planner with RRF fusion across grep,
// AST, and (when enabled) the centroid vector index.
func (c *Client) Search(ctx context.Context, uri, query string, opts *SearchOptions) ([]Span, error) {
	body := map[string]any{"uri": uri, "query": query}
	mergeOptions(body, opts)
	var out SpansResponse
	if err := c.post(ctx, "/search", body, &out); err != nil {
		return nil, err
	}
	return out.Spans, nil
}

// FindSymbol issues a /find_symbol request: tree-sitter-only symbol lookup.
func (c *Client) FindSymbol(ctx context.Context, uri, name string, opts *FindSymbolOptions) ([]Span, error) {
	body := map[string]any{"uri": uri, "symbol": name}
	mergeOptions(body, opts)
	var out SpansResponse
	if err := c.post(ctx, "/find_symbol", body, &out); err != nil {
		return nil, err
	}
	return out.Spans, nil
}

// Ls issues a /ls request: listing of objects under a URI prefix.
func (c *Client) Ls(ctx context.Context, uri string, opts *LsOptions) ([]Entry, error) {
	body := map[string]any{"uri": uri}
	mergeOptions(body, opts)
	var out ListResponse
	if err := c.post(ctx, "/ls", body, &out); err != nil {
		return nil, err
	}
	return out.Entries, nil
}

// Read issues a /read request: full-object fetch through the tier cache.
func (c *Client) Read(ctx context.Context, uri string) (*ReadResponse, error) {
	var out ReadResponse
	if err := c.post(ctx, "/read", map[string]any{"uri": uri}, &out); err != nil {
		return nil, err
	}
	return &out, nil
}

func (c *Client) post(ctx context.Context, path string, body any, out any) error {
	b, err := json.Marshal(body)
	if err != nil {
		return fmt.Errorf("agenticsearch: marshal request: %w", err)
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, c.baseURL+path, bytes.NewReader(b))
	if err != nil {
		return fmt.Errorf("agenticsearch: build request: %w", err)
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Accept", "application/json")

	resp, err := c.http.Do(req)
	if err != nil {
		return fmt.Errorf("agenticsearch: %s %s: %w", req.Method, path, err)
	}
	defer resp.Body.Close()

	if resp.StatusCode >= 400 {
		buf, _ := io.ReadAll(io.LimitReader(resp.Body, 8192))
		return fmt.Errorf("agenticsearch: %s %s: %s: %s", req.Method, path, resp.Status, string(buf))
	}
	if out == nil {
		return nil
	}
	if err := json.NewDecoder(resp.Body).Decode(out); err != nil {
		return fmt.Errorf("agenticsearch: decode response: %w", err)
	}
	return nil
}

// mergeOptions copies non-zero json-tagged fields from opts (a struct
// pointer) onto body. Skips nil pointers and any field whose tag is "-".
func mergeOptions(body map[string]any, opts any) {
	if opts == nil {
		return
	}
	b, err := json.Marshal(opts)
	if err != nil {
		return
	}
	var m map[string]any
	if err := json.Unmarshal(b, &m); err != nil {
		return
	}
	for k, v := range m {
		body[k] = v
	}
}
