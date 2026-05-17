#!/usr/bin/env python3
"""Macro benchmark harness for agentic-search.

Compares:
  - agentic-search grep                         (parallel ripgrep-as-library, no AST)
  - agentic-search grep --ast                   (+ tree-sitter span widening)
  - rg                                          (subprocess to native ripgrep)
  - probe (probelabs/probe)                     (skipped if not on PATH)

Corpus: this script downloads + extracts a fixed slice of source code to
``bench/data/corpus`` (tokio-rs / tokio so the result mix has plenty of
Rust function definitions). Then runs each search engine N times and
reports p50, p95, and result count.

Usage::

    cargo build --release -p as-cli
    python bench/macro/run.py --runs 5
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import tarfile
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
DATA = ROOT / "bench" / "data"
CORPUS = DATA / "corpus"
RESULTS = ROOT / "bench" / "results"
BIN = ROOT / "target" / "release" / "agentic-search"
CORPUS_URL = "https://github.com/tokio-rs/tokio/archive/refs/tags/tokio-1.40.0.tar.gz"
CORPUS_NAME = "tokio-tokio-1.40.0"


def ensure_corpus() -> Path:
    DATA.mkdir(parents=True, exist_ok=True)
    target = CORPUS / CORPUS_NAME
    if target.exists():
        return target
    archive = DATA / "tokio.tar.gz"
    if not archive.exists():
        print(f"downloading corpus from {CORPUS_URL}", flush=True)
        urllib.request.urlretrieve(CORPUS_URL, archive)
    CORPUS.mkdir(parents=True, exist_ok=True)
    with tarfile.open(archive) as tf:
        tf.extractall(CORPUS)
    return target


def have(binary: str) -> bool:
    return shutil.which(binary) is not None


def time_call(args: list[str], runs: int) -> tuple[list[float], int, int]:
    durations: list[float] = []
    out_bytes = 0
    last_rc = 0
    for _ in range(runs):
        t0 = time.perf_counter()
        r = subprocess.run(args, capture_output=True)
        durations.append(time.perf_counter() - t0)
        out_bytes = len(r.stdout)
        last_rc = r.returncode
    return durations, out_bytes, last_rc


def percentile(values: list[float], p: float) -> float:
    if not values:
        return float("nan")
    s = sorted(values)
    k = max(0, min(len(s) - 1, int(round((p / 100) * (len(s) - 1)))))
    return s[k]


def fmt_ms(seconds: float) -> str:
    return f"{seconds * 1000:.1f}ms"


def upload_to_rustfs(corpus: Path, bucket: str, prefix: str) -> str:
    """Upload corpus into a RustFS bucket and return the s3:// URI.

    Caller is responsible for ``scripts/rustfs-up.sh`` already running.
    """
    os.environ.setdefault("AWS_ACCESS_KEY_ID", "testkey")
    os.environ.setdefault("AWS_SECRET_ACCESS_KEY", "testsecret")
    os.environ.setdefault("AWS_REGION", "us-east-1")
    endpoint = os.environ.get("AWS_ENDPOINT_URL", "http://localhost:19000")
    # idempotent: skip if already present
    head = subprocess.run(
        ["aws", "--endpoint-url", endpoint, "s3", "ls", f"s3://{bucket}/{prefix}/"],
        capture_output=True,
    )
    if head.returncode != 0 or not head.stdout.strip():
        subprocess.run(
            ["aws", "--endpoint-url", endpoint, "s3", "cp", str(corpus), f"s3://{bucket}/{prefix}/", "--recursive"],
            check=True,
            capture_output=True,
        )
    return f"s3://{bucket}/{prefix}"


def bench_server(
    corpus: Path,
    pattern: str,
    runs: int,
    max_hits: int,
) -> list[dict]:
    """Bench `grep` and `grep --ast` against a persistent HTTP server.

    Captures the AST parse cache benefit (the CLI path opens a fresh
    cache per invocation so it always pays parse cost)."""
    import socket
    import time
    import urllib.request

    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
    bind = f"127.0.0.1:{port}"
    server = subprocess.Popen(
        [str(BIN), "serve", "--bind", bind],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    base = f"http://{bind}"
    try:
        deadline = time.time() + 10.0
        while time.time() < deadline:
            try:
                with urllib.request.urlopen(f"{base}/health", timeout=1):
                    break
            except Exception:
                time.sleep(0.1)

        def post(path: str, body: dict) -> bytes:
            data = json.dumps(body).encode()
            req = urllib.request.Request(
                f"{base}{path}",
                data=data,
                headers={"content-type": "application/json"},
            )
            with urllib.request.urlopen(req, timeout=60) as r:
                return r.read()

        def time_runs(body: dict, n: int) -> list[float]:
            # Single warm-up run discarded: the AST cache, tier cache,
            # and tokio runtime all amortise across the same prefix
            # after the first request. Without this the "warm AST
            # cache" label is misleading — run #1 paid the cold parse
            # cost and contaminated p50/p95/mean.
            try:
                post("/grep", body)
            except Exception:
                pass
            durations = []
            payload_len = 0
            for _ in range(n):
                t0 = time.perf_counter()
                payload = post("/grep", body)
                durations.append(time.perf_counter() - t0)
                payload_len = len(payload)
            return durations, payload_len

        rows = []
        uri = f"file://{corpus}"
        for label, ast in (
            ("agentic-search /grep (server)", False),
            ("agentic-search /grep --ast (server, warm AST cache)", True),
        ):
            durations, payload_len = time_runs(
                {
                    "uri": uri,
                    "pattern": pattern,
                    "max_hits": max_hits,
                    "concurrency": 64,
                    "ast": ast,
                },
                runs,
            )
            rows.append(
                {
                    "engine": label,
                    "p50_ms": round(percentile(durations, 50) * 1000, 2),
                    "p95_ms": round(percentile(durations, 95) * 1000, 2),
                    "mean_ms": round(statistics.fmean(durations) * 1000, 2),
                    "out_bytes": payload_len,
                    "rc": 0,
                }
            )
        return rows
    finally:
        server.terminate()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--pattern", default=r"async fn")
    ap.add_argument("--max-hits", type=int, default=5000)
    ap.add_argument(
        "--s3",
        action="store_true",
        help="Also benchmark against a local RustFS S3 endpoint",
    )
    ap.add_argument(
        "--server",
        action="store_true",
        help="Also benchmark against a persistent agentic-search server (captures AST cache hits)",
    )
    ap.add_argument("--s3-bucket", default="agentic-search-it")
    ap.add_argument("--s3-prefix", default="tokio")
    args = ap.parse_args()

    if not BIN.exists():
        print(f"binary missing: {BIN}; run cargo build --release -p as-cli", file=sys.stderr)
        return 2

    corpus = ensure_corpus()
    uri = f"file://{corpus}"

    print(f"corpus      : {corpus}")
    n_files = sum(1 for _ in corpus.rglob("*") if _.is_file())
    print(f"files       : {n_files}")
    print(f"pattern     : {args.pattern!r}")
    print(f"runs        : {args.runs}")
    print()

    engines: list[tuple[str, list[str]]] = [
        (
            "agentic-search grep (local)",
            [str(BIN), "grep", uri, args.pattern, "--max-hits", str(args.max_hits), "--concurrency", "64"],
        ),
        (
            "agentic-search grep --ast (local)",
            [str(BIN), "grep", uri, args.pattern, "--max-hits", str(args.max_hits), "--concurrency", "64", "--ast"],
        ),
        ("rg (subprocess)", ["rg", "--no-heading", "-n", args.pattern, str(corpus)]),
    ]
    if have("probe"):
        engines.append(("probe search", ["probe", "search", args.pattern, str(corpus)]))
    else:
        print("note: `probe` not on PATH; skipping probelabs/probe comparison")
        print("      install via: pnpm add -g @probelabs/probe\n")

    if args.s3:
        os.environ.setdefault("AWS_ENDPOINT_URL", "http://localhost:19000")
        os.environ.setdefault("AWS_VIRTUAL_HOSTED_STYLE_REQUEST", "false")
        os.environ.setdefault("AWS_ALLOW_HTTP", "true")
        s3_uri = upload_to_rustfs(corpus, args.s3_bucket, args.s3_prefix)
        print(f"s3 uri      : {s3_uri}")
        engines.append(
            (
                "agentic-search grep (s3 cold/warm-mixed)",
                [str(BIN), "grep", s3_uri, args.pattern, "--max-hits", str(args.max_hits), "--concurrency", "64"],
            )
        )
        engines.append(
            (
                "agentic-search grep --ast (s3)",
                [str(BIN), "grep", s3_uri, args.pattern, "--max-hits", str(args.max_hits), "--concurrency", "64", "--ast"],
            )
        )

    rows = []
    for name, cmd in engines:
        durations, out_bytes, rc = time_call(cmd, args.runs)
        rows.append(
            {
                "engine": name,
                "p50_ms": round(percentile(durations, 50) * 1000, 2),
                "p95_ms": round(percentile(durations, 95) * 1000, 2),
                "mean_ms": round(statistics.fmean(durations) * 1000, 2),
                "out_bytes": out_bytes,
                "rc": rc,
            }
        )
    if args.server:
        rows.extend(bench_server(corpus, args.pattern, args.runs, args.max_hits))

    width_name = max(len(r["engine"]) for r in rows)
    print(f"{'engine'.ljust(width_name)}  {'p50':>10}  {'p95':>10}  {'mean':>10}  {'out bytes':>12}  rc")
    for r in rows:
        print(
            f"{r['engine'].ljust(width_name)}  "
            f"{r['p50_ms']:>8.1f}ms  "
            f"{r['p95_ms']:>8.1f}ms  "
            f"{r['mean_ms']:>8.1f}ms  "
            f"{r['out_bytes']:>12}  {r['rc']}"
        )

    RESULTS.mkdir(parents=True, exist_ok=True)
    out = {
        "corpus": str(corpus),
        "files": n_files,
        "pattern": args.pattern,
        "runs": args.runs,
        "platform": sys.platform,
        "results": rows,
    }
    stamp = time.strftime("%Y-%m-%dT%H-%M-%S")
    out_path = RESULTS / f"{stamp}.json"
    out_path.write_text(json.dumps(out, indent=2))
    print(f"\nwrote {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
