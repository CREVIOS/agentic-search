package agenticsearch

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestGrepRoundtrip(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/grep" {
			t.Fatalf("unexpected path: %s", r.URL.Path)
		}
		var body map[string]any
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			t.Fatalf("bad body: %v", err)
		}
		if body["uri"] != "file:///tmp" || body["pattern"] != "TODO" {
			t.Fatalf("unexpected body: %+v", body)
		}
		// also assert that GrepOptions with non-zero AST made it through
		if body["ast"] != true {
			t.Fatalf("ast option not propagated: %+v", body)
		}
		_ = json.NewEncoder(w).Encode(SpansResponse{
			Spans: []Span{{URI: "file:///tmp/a.rs", Kind: SpanLine, Snippet: "// TODO"}},
		})
	}))
	defer srv.Close()

	c := New(srv.URL)
	spans, err := c.Grep(context.Background(), "file:///tmp", "TODO", &GrepOptions{AST: true})
	if err != nil {
		t.Fatalf("grep: %v", err)
	}
	if len(spans) != 1 || spans[0].URI != "file:///tmp/a.rs" {
		t.Fatalf("unexpected spans: %+v", spans)
	}
}

func TestFindSymbolHitsFindRoute(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/find" {
			t.Fatalf("FindSymbol must POST /find, got %s", r.URL.Path)
		}
		_ = json.NewEncoder(w).Encode(SpansResponse{Spans: []Span{{Symbol: "verify_jwt"}}})
	}))
	defer srv.Close()

	c := New(srv.URL)
	spans, err := c.FindSymbol(context.Background(), "s3://x/", "verify_jwt", nil)
	if err != nil {
		t.Fatalf("find: %v", err)
	}
	if len(spans) != 1 || spans[0].Symbol != "verify_jwt" {
		t.Fatalf("unexpected: %+v", spans)
	}
}

func TestReadDecodesTextField(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		_, _ = w.Write([]byte(`{"uri":"file:///x/a.txt","bytes":5,"text":"hello"}`))
	}))
	defer srv.Close()

	c := New(srv.URL)
	resp, err := c.Read(context.Background(), "file:///x/a.txt")
	if err != nil {
		t.Fatalf("read: %v", err)
	}
	if resp.Bytes != 5 || resp.Text == nil || *resp.Text != "hello" {
		t.Fatalf("unexpected: %+v", resp)
	}
}

func TestErrorPropagation(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
		http.Error(w, "bad uri", http.StatusBadRequest)
	}))
	defer srv.Close()

	c := New(srv.URL)
	_, err := c.Search(context.Background(), "", "x", nil)
	if err == nil {
		t.Fatal("expected error for 400 response")
	}
}
