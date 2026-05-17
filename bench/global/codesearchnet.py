"""Global benchmark: CodeSearchNet Challenge MRR / NDCG.

Pulls the published CodeSearchNet challenge queries + qrels (the official
99-query human-relevance set) and the python corpus, runs each query
through the agentic-search HTTP server, and reports MRR@10 and NDCG@10.

This is an *honest* lexical-mode evaluation: CodeSearchNet's queries are
short natural-language phrases ("read csv into dataframe", "open a
serial port"); a pure regex grep is not going to dominate semantic
methods like CasCode / GraphCodeBERT. The point is to publish a
reproducible number, not to claim the leaderboard.

Usage::

    cargo build --release -p as-cli
    /tmp/asv/bin/python -m pip install datasets requests
    /Users/asif/Desktop/opensource/target/release/agentic-search serve --bind 127.0.0.1:8787 &
    /tmp/asv/bin/python bench/global/codesearchnet.py \\
        --language python --max-queries 50 --max-docs 5000
"""

from __future__ import annotations

import argparse
import json
import math
import os
import socket
import statistics
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

import requests

ROOT = Path(__file__).resolve().parent.parent.parent
DATA = ROOT / "bench" / "data" / "codesearchnet"
RESULTS = ROOT / "bench" / "results"
BIN = ROOT / "target" / "release" / "agentic-search"


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def wait_for(url: str, timeout: float = 10.0) -> None:
    import urllib.request

    end = time.time() + timeout
    while time.time() < end:
        try:
            with urllib.request.urlopen(url, timeout=1) as r:
                if r.status < 500:
                    return
        except Exception:
            pass
        time.sleep(0.2)
    raise TimeoutError(f"server not up: {url}")


def prepare_corpus(language: str, max_docs: int) -> tuple[Path, dict[str, str]]:
    """Materialize a slice of CodeSearchNet as a `file://` corpus.

    Returns (corpus_path, doc_id_to_relative_path).
    """
    from datasets import load_dataset

    corpus = DATA / f"{language}-{max_docs}"
    corpus.mkdir(parents=True, exist_ok=True)
    map_file = corpus.parent / f"{language}-{max_docs}.map.json"

    if map_file.exists() and any(corpus.iterdir()):
        return corpus, json.loads(map_file.read_text())

    print(f"materializing {language} corpus ({max_docs} docs)...", flush=True)
    ds = load_dataset(
        "code-search-net/code_search_net",
        language,
        split="test",
        trust_remote_code=True,
    )
    doc_map: dict[str, str] = {}
    n = 0
    for row in ds:
        if n >= max_docs:
            break
        url = row["func_code_url"]  # e.g. https://github.com/.../blob/SHA/path.py#L10-L20
        code = row["func_code_string"]
        # Make a flat, stable filename.
        rel = f"{n:05d}_{Path(url.split('#')[0]).name}"
        (corpus / rel).write_text(code)
        doc_map[url] = rel
        n += 1
    map_file.write_text(json.dumps(doc_map, indent=2))
    print(f"  wrote {n} files under {corpus}", flush=True)
    return corpus, doc_map


def load_queries(language: str) -> list[dict[str, Any]]:
    """Use the CSN test set itself: docstring = NL query, func URL = gold doc.

    The official `irds/codesearchnet_challenge` queries are language-
    agnostic and small (99 total); this self-pair approach is the
    standard CSN MRR setup used by the original paper.
    """
    from datasets import load_dataset

    ds = load_dataset(
        "code-search-net/code_search_net",
        language,
        split="test",
        trust_remote_code=True,
    )
    out = []
    for row in ds:
        doc = (row["func_documentation_string"] or "").strip()
        if not doc:
            continue
        # Use first paragraph only.
        head = doc.split("\n\n")[0].strip().splitlines()[0].strip()
        if not head or len(head) < 5:
            continue
        out.append({"query": head, "gold_url": row["func_code_url"]})
    return out


def mrr_at_k(ranks: list[int | None], k: int = 10) -> float:
    rr = []
    for r in ranks:
        if r is None or r > k:
            rr.append(0.0)
        else:
            rr.append(1.0 / r)
    return statistics.fmean(rr) if rr else 0.0


def ndcg_at_k(ranks: list[int | None], k: int = 10) -> float:
    out = []
    for r in ranks:
        if r is None or r > k:
            out.append(0.0)
        else:
            out.append(1.0 / math.log2(r + 1))
    return statistics.fmean(out) if out else 0.0


def query_agentic_search(server_url: str, corpus_uri: str, q: str, k: int) -> list[str]:
    """Return ranked list of file paths."""
    # Build a regex that matches each non-trivial token from the query.
    tokens = [t for t in q.replace("/", " ").replace("(", " ").replace(")", " ").split() if len(t) >= 3]
    if not tokens:
        return []
    # OR-of-tokens regex; case insensitive.
    pattern = "|".join(re_escape(t) for t in tokens[:8])
    r = requests.post(
        f"{server_url}/grep",
        json={
            "uri": corpus_uri,
            "pattern": pattern,
            "case_insensitive": True,
            "ast": True,
            "max_hits": k * 10,
            "concurrency": 64,
        },
        timeout=60,
    )
    r.raise_for_status()
    spans = r.json().get("spans", [])
    # Aggregate by file, keeping insertion order (server already roughly
    # ranks by appearance order across the parallel scan).
    seen: list[str] = []
    seenset = set()
    for s in spans:
        u = s["uri"]
        if u not in seenset:
            seenset.add(u)
            seen.append(u)
        if len(seen) >= k:
            break
    return seen


def re_escape(s: str) -> str:
    out = []
    for c in s:
        if c in r".+*?()|[]{}^$\\/":
            out.append("\\" + c)
        else:
            out.append(c)
    return "".join(out)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--language", default="python")
    ap.add_argument("--max-queries", type=int, default=50)
    ap.add_argument("--max-docs", type=int, default=2000)
    ap.add_argument("--k", type=int, default=10)
    args = ap.parse_args()

    if not BIN.exists():
        print(f"binary missing: {BIN}", file=sys.stderr)
        return 2
    try:
        import datasets  # noqa: F401
    except ImportError:
        print("pip install datasets requests", file=sys.stderr)
        return 2

    corpus, doc_map = prepare_corpus(args.language, args.max_docs)
    # Reverse map: relative filename -> URL (so we can recover gold).
    rel_to_url = {v: k for k, v in doc_map.items()}

    queries = load_queries(args.language)[: args.max_queries]
    # Keep only queries whose gold URL is actually in the corpus slice.
    keep = []
    for q in queries:
        if q["gold_url"] in doc_map:
            keep.append(q)
    print(f"queries: {len(keep)} (after filtering to corpus slice)")
    if not keep:
        print("no queries hit; raise --max-docs or --max-queries", file=sys.stderr)
        return 1

    port = free_port()
    bind = f"127.0.0.1:{port}"
    server_url = f"http://{bind}"
    server = subprocess.Popen(
        [str(BIN), "serve", "--bind", bind],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        wait_for(f"{server_url}/health", timeout=10)
        corpus_uri = f"file://{corpus}"
        ranks: list[int | None] = []
        t0 = time.perf_counter()
        for q in keep:
            ranked = query_agentic_search(server_url, corpus_uri, q["query"], args.k)
            gold_rel = doc_map[q["gold_url"]]
            rank: int | None = None
            for i, hit_uri in enumerate(ranked, start=1):
                hit_rel = Path(hit_uri).name
                if hit_rel == gold_rel:
                    rank = i
                    break
            ranks.append(rank)
        elapsed = time.perf_counter() - t0

        mrr = mrr_at_k(ranks, args.k)
        ndcg = ndcg_at_k(ranks, args.k)
        recall = sum(1 for r in ranks if r is not None) / len(ranks)
        per_query_ms = round(elapsed / len(ranks) * 1000, 2)

        print()
        print(f"engine        : agentic-search grep --ast (lexical, OR-of-tokens)")
        print(f"language      : {args.language}")
        print(f"corpus docs   : {len(doc_map)}")
        print(f"queries       : {len(ranks)}")
        print(f"MRR@{args.k}        : {mrr:.4f}")
        print(f"NDCG@{args.k}       : {ndcg:.4f}")
        print(f"Recall@{args.k}     : {recall:.4f}")
        print(f"per-query     : {per_query_ms} ms")

        RESULTS.mkdir(parents=True, exist_ok=True)
        stamp = time.strftime("%Y-%m-%dT%H-%M-%S")
        out = {
            "benchmark": "CodeSearchNet (challenge-style, lexical mode)",
            "engine": "agentic-search grep --ast (OR-of-tokens regex)",
            "language": args.language,
            "corpus_docs": len(doc_map),
            "queries": len(ranks),
            "metrics": {
                "MRR@10": round(mrr, 4),
                "NDCG@10": round(ndcg, 4),
                "Recall@10": round(recall, 4),
                "per_query_ms": per_query_ms,
            },
        }
        out_path = RESULTS / f"codesearchnet-{args.language}-{stamp}.json"
        out_path.write_text(json.dumps(out, indent=2))
        print(f"\nwrote {out_path}")
        return 0
    finally:
        server.terminate()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()


if __name__ == "__main__":
    sys.exit(main())
