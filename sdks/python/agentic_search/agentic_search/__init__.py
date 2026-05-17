"""Native Python client for the agentic-search REST server.

No MCP. No agent framework. Just a thin HTTP client over the
`agentic-search serve` REST surface. Use it from any Python program,
notebook, FastAPI handler, Airflow task, cron job — whatever already
speaks HTTP.

Example::

    from agentic_search import Client

    c = Client("http://127.0.0.1:8787")
    hits = c.grep("s3://my-corpus/", "TODO\\(security\\)", ast=True)
    for s in hits:
        print(s.uri, s.line_range[0], s.snippet)

    # Hybrid grep + AST + (optional) centroid vector
    spans = c.search("s3://my-corpus/", "graceful shutdown", k=10)

    # Read by exact URI returned by grep/search
    body = c.read(spans[0].uri).text

The wire shape is identical to the MCP tool surface; this is the
non-MCP path for callers that don't need or want a JSON-RPC stdio
transport.
"""

from __future__ import annotations

import dataclasses
import json
from dataclasses import dataclass
from typing import Any

import requests

__all__ = ["Client", "Span", "ReadResponse", "Entry"]


@dataclass
class Span:
    """One match returned by grep / search / find."""

    uri: str
    line_range: tuple[int, int]
    byte_range: tuple[int, int]
    kind: str
    snippet: str | None
    score: float
    symbol: str | None = None
    source_stage: str | None = None
    content_hash: str | None = None

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> "Span":
        br = j.get("byte_range") or {}
        if isinstance(br, dict):
            br_t = (int(br.get("start", 0)), int(br.get("end", 0)))
        else:
            br_t = (int(br[0]), int(br[1]))
        return cls(
            uri=j["uri"],
            line_range=tuple(j.get("line_range", [1, 1])),  # type: ignore[arg-type]
            byte_range=br_t,
            kind=j.get("kind", "line"),
            snippet=j.get("snippet"),
            score=float(j.get("score", 0.0)),
            symbol=j.get("symbol"),
            source_stage=j.get("source_stage"),
            content_hash=j.get("content_hash"),
        )


@dataclass
class ReadResponse:
    uri: str
    bytes: int
    text: str | None


@dataclass
class Entry:
    key: str
    size: int
    last_modified: int | None = None


class Client:
    """Native Python client over the agentic-search REST surface.

    Thread-safe; one `requests.Session` per client. Reuse a single
    Client across an application — sessions pool connections.

    Parameters
    ----------
    base_url:
        Where the server is bound. Defaults to the loopback bind
        (`http://127.0.0.1:8787`) that `agentic-search serve` ships
        with.
    timeout:
        Per-request timeout in seconds. Default 30.
    """

    def __init__(self, base_url: str = "http://127.0.0.1:8787", timeout: float = 30.0):
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout
        self._s = requests.Session()

    # ---- core verbs ----

    def grep(
        self,
        uri: str,
        pattern: str,
        *,
        case_insensitive: bool = False,
        max_hits: int = 200,
        concurrency: int = 32,
        ast: bool = False,
    ) -> list[Span]:
        body = {
            "uri": uri,
            "pattern": pattern,
            "case_insensitive": case_insensitive,
            "max_hits": max_hits,
            "concurrency": concurrency,
            "ast": ast,
        }
        return self._spans("/grep", body)

    def search(self, uri: str, query: str, *, k: int = 20) -> list[Span]:
        return self._spans("/search", {"uri": uri, "query": query, "k": k})

    def find_symbol(
        self,
        uri: str,
        symbol: str,
        *,
        max_hits: int = 20,
        concurrency: int = 4,
    ) -> list[Span]:
        return self._spans(
            "/find",
            {
                "uri": uri,
                "symbol": symbol,
                "max_hits": max_hits,
                "concurrency": concurrency,
            },
        )

    def ls(self, uri: str, *, glob: str | None = None, limit: int = 1000) -> list[Entry]:
        body: dict[str, Any] = {"uri": uri, "limit": limit}
        if glob:
            body["glob"] = glob
        data = self._post("/ls", body)
        return [
            Entry(
                key=e["key"],
                size=int(e.get("size", 0)),
                last_modified=e.get("last_modified"),
            )
            for e in data.get("entries", [])
        ]

    def read(
        self,
        uri: str,
        *,
        offset: int | None = None,
        length: int | None = None,
    ) -> ReadResponse:
        body: dict[str, Any] = {"uri": uri}
        if offset is not None and length is not None:
            body["offset"] = offset
            body["length"] = length
        data = self._post("/read", body)
        return ReadResponse(
            uri=data.get("uri", uri),
            bytes=int(data.get("bytes", 0)),
            text=data.get("text"),
        )

    @staticmethod
    def join_uri(corpus_uri: str, key: str) -> str:
        """Reassemble a span's prefix-relative ``uri`` back into a full
        ``scheme://host/key`` so it can be handed to :meth:`read`.

        ``Span.uri`` from grep / search is **prefix-relative** by
        design (smaller payloads, less per-span repetition). This
        helper does the join so caller code stays one-line::

            spans = c.grep("s3://bucket/corpus", "TODO")
            body = c.read(Client.join_uri("s3://bucket/corpus", spans[0].uri))
        """
        if "://" in key:
            return key
        scheme, _, host_path = corpus_uri.partition("://")
        host, _, prefix = host_path.partition("/")
        prefix = prefix.rstrip("/")
        if prefix and key.startswith(prefix + "/"):
            key = key[len(prefix) + 1 :]
        joined = f"{prefix}/{key}" if prefix else key
        return f"{scheme}://{host}/{joined}"

    def health(self) -> bool:
        try:
            r = self._s.get(f"{self.base_url}/health", timeout=self.timeout)
            return r.ok
        except Exception:
            return False

    # ---- internals ----

    def _post(self, path: str, body: dict[str, Any]) -> dict[str, Any]:
        r = self._s.post(f"{self.base_url}{path}", json=body, timeout=self.timeout)
        if not r.ok:
            raise RuntimeError(f"agentic-search {path}: {r.status_code} {r.text[:400]}")
        return r.json()

    def _spans(self, path: str, body: dict[str, Any]) -> list[Span]:
        return [Span.from_json(s) for s in self._post(path, body).get("spans", [])]

    def close(self) -> None:
        self._s.close()

    def __enter__(self) -> "Client":
        return self

    def __exit__(self, *_: Any) -> None:
        self.close()
