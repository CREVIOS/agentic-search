/**
 * Node SDK for agentic-search.
 *
 * Thin client over the HTTP server exposed by `agentic-search serve`.
 * Mirrors the server's REST contract: `uri` is required on every call,
 * results come back as `spans`.
 */

export type SpanKind = "line" | "block" | "function" | "method" | "class" | "module";

export interface Span {
  uri: string;
  byte_range: { start: number; end: number };
  line_range: [number, number];
  symbol?: string;
  kind: SpanKind;
  snippet?: string;
  score: number;
}

export interface GrepOptions {
  case_insensitive?: boolean;
  max_hits?: number;
  concurrency?: number;
  /** Widen each line hit to its enclosing function/class/method via tree-sitter. */
  ast?: boolean;
}

export interface SearchOptions {
  k?: number;
}

export interface FindSymbolOptions {
  max_hits?: number;
  concurrency?: number;
}

export interface LsOptions {
  glob?: string;
  limit?: number;
}

export class AgenticSearchClient {
  constructor(private readonly baseUrl: string = "http://127.0.0.1:8787") {}

  async ls(uri: string, opts: LsOptions = {}): Promise<{ key: string; size: number }[]> {
    const data = await this.post<{ entries: { key: string; size: number }[] }>("/ls", {
      uri,
      glob: opts.glob,
      limit: opts.limit ?? 1000,
    });
    return data.entries;
  }

  async read(
    uri: string,
    range?: { offset: number; length: number }
  ): Promise<{ uri: string; bytes: number; text?: string }> {
    return this.post("/read", {
      uri,
      offset: range?.offset,
      length: range?.length,
    });
  }

  async grep(uri: string, pattern: string, opts: GrepOptions = {}): Promise<Span[]> {
    const data = await this.post<{ spans: Span[] }>("/grep", {
      uri,
      pattern,
      case_insensitive: opts.case_insensitive ?? false,
      max_hits: opts.max_hits ?? 1000,
      concurrency: opts.concurrency ?? 32,
      ast: opts.ast ?? false,
    });
    return data.spans;
  }

  async findSymbol(uri: string, symbol: string, opts: FindSymbolOptions = {}): Promise<Span[]> {
    const data = await this.post<{ spans: Span[] }>("/find", {
      uri,
      symbol,
      max_hits: opts.max_hits ?? 200,
      concurrency: opts.concurrency ?? 32,
    });
    return data.spans;
  }

  async search(uri: string, query: string, opts: SearchOptions = {}): Promise<Span[]> {
    const data = await this.post<{ spans: Span[] }>("/search", {
      uri,
      query,
      k: opts.k ?? 20,
    });
    return data.spans;
  }

  private async post<T>(path: string, body: unknown): Promise<T> {
    const res = await fetch(`${this.baseUrl}${path}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
    if (!res.ok) {
      const text = await res.text().catch(() => "");
      throw new Error(`agentic-search ${path}: ${res.status} ${res.statusText} ${text}`);
    }
    return (await res.json()) as T;
  }
}
