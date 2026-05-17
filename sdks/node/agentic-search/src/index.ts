/**
 * Node SDK for agentic-search.
 *
 * Thin client over the HTTP server exposed by `agentic-search serve`.
 */

export interface Hit {
  id: string;
  uri: string;
  score: number;
  snippet?: string;
  metadata?: unknown;
}

export interface SearchOptions {
  k?: number;
}

export class AgenticSearchClient {
  constructor(private readonly baseUrl: string = "http://127.0.0.1:8787") {}

  async search(query: string, opts: SearchOptions = {}): Promise<Hit[]> {
    const res = await fetch(`${this.baseUrl}/search`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ query, k: opts.k ?? 10 }),
    });
    if (!res.ok) {
      throw new Error(`agentic-search: ${res.status} ${res.statusText}`);
    }
    const data = (await res.json()) as { hits: Hit[] };
    return data.hits;
  }
}
