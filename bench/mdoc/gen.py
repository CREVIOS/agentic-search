#!/usr/bin/env python3
"""Generate a million-document synthetic corpus for scale benchmarks.

Two output modes:

  --mode text    Plain text files under <out>/docs/<shard>/<id>.md.
                 Each doc is 200-2000 chars of word salad with a
                 fixed vocabulary so grep regex hits are findable.

  --mode vec     Pre-built centroid index laid out per the as-vec
                 manifest format. Vectors are NOT real embeddings —
                 they are synthetic K-cluster gaussians with controlled
                 cluster purity so kmeans-on-disk actually finds the
                 ground-truth centroids and we can measure recall.

A 1M-doc text run produces ~600 MB on disk in ~90 s on an NVMe SSD.
A 1M-vector index run produces ~1.6 GB (1M * 384 * 4 + manifest +
centroid header) in ~120 s, single-threaded numpy.

Usage::

    python bench/mdoc/gen.py --mode text --n 1000000 --out /tmp/mdoc
    python bench/mdoc/gen.py --mode vec  --n 1000000 --k 1024 \\
        --out /tmp/mdoc-vec
"""

from __future__ import annotations

import argparse
import json
import os
import random
import struct
import sys
import time
from pathlib import Path

VOCAB = (
    "alpha beta gamma delta epsilon zeta eta theta iota kappa "
    "lambda mu nu xi omicron pi rho sigma tau upsilon phi chi psi omega "
    "agent search index vector grep span token cache lookup query plan "
    "fast slow read write fetch parse score rank rerank fuse hybrid "
    "todo fixme hack bug perf note warn error info debug trace mark "
).split()

SENTENCES = [
    "the {a} {b} {c} reached {d} {e} after {f} ticks",
    "{a} {b} hit a {c} during {d}; needs investigation",
    "perf: {a} {b} regressed from {c}ms to {d}ms on the {e} path",
    "TODO: rewrite the {a} {b} loop in terms of {c}",
    "TODO(security): {a} {b} should validate {c} before {d}",
    "fix(scope): {a} {b} now uses {c} instead of {d}",
    "internal note: {a} {b} is the new {c}; deprecate {d}",
]


def text_doc(rng: random.Random) -> bytes:
    """Compose 5-30 sentences of word-salad with predictable hit terms."""
    n = rng.randint(5, 30)
    lines = []
    for _ in range(n):
        template = rng.choice(SENTENCES)
        filled = template.format(
            a=rng.choice(VOCAB),
            b=rng.choice(VOCAB),
            c=rng.choice(VOCAB),
            d=rng.choice(VOCAB),
            e=rng.choice(VOCAB),
            f=rng.randint(1, 9999),
        )
        lines.append(filled)
    return ("\n".join(lines) + "\n").encode("utf-8")


def gen_text(out: Path, n: int, shard_size: int, seed: int) -> None:
    rng = random.Random(seed)
    out.mkdir(parents=True, exist_ok=True)
    docs_root = out / "docs"
    docs_root.mkdir(exist_ok=True)
    started = time.time()
    written = 0
    bytes_written = 0
    for i in range(n):
        shard = f"{(i // shard_size):05d}"
        shard_dir = docs_root / shard
        if i % shard_size == 0:
            shard_dir.mkdir(parents=True, exist_ok=True)
        path = shard_dir / f"{i:08d}.md"
        body = text_doc(rng)
        path.write_bytes(body)
        bytes_written += len(body)
        written += 1
        if written % 50_000 == 0:
            elapsed = time.time() - started
            print(
                f"  {written:>9}/{n} docs  "
                f"({bytes_written / 1e6:.0f} MB, "
                f"{written / elapsed:,.0f} docs/s)",
                flush=True,
            )
    print(
        f"text: wrote {written} docs, {bytes_written / 1e6:.0f} MB "
        f"in {time.time() - started:.1f}s"
    )


# --- vector index ---

import numpy as np  # noqa: E402  (lazy import — text mode doesn't need it)


def gen_vec(out: Path, n: int, k: int, dim: int, seed: int) -> None:
    """Lay out an as-vec compatible index on disk.

    Manifest format (matches crates/as-vec/src/manifest.rs):
        manifest.json    : version, dim, k, embed_model, num_docs,
                           centroids_file, docs_file, cluster_files[],
                           cluster_sizes[]
        centroids.f32    : k * dim f32 little-endian
        cluster_<id>.bin : per-cluster records:
                              [u32 count]
                              repeat count times:
                                  u32 doc_id
                                  dim f32 vector
        docs.jsonl       : one JSON object per doc, ordered by doc_id:
                              { "doc_id", "uri", "byte_range", "snippet" }
    """
    out.mkdir(parents=True, exist_ok=True)
    rng = np.random.default_rng(seed)

    print(f"vec: generating {n} vectors in dim={dim}, k={k}")
    centroids = rng.standard_normal((k, dim)).astype(np.float32)
    centroids /= np.linalg.norm(centroids, axis=1, keepdims=True) + 1e-9

    # Assign each doc to a cluster, then jitter around its centroid.
    # Controlled purity: 0.85 close to centroid + 0.15 random noise.
    assignments = rng.integers(0, k, size=n).astype(np.uint32)
    noise = rng.standard_normal((n, dim)).astype(np.float32) * 0.15
    vecs = centroids[assignments] * 0.85 + noise
    vecs /= np.linalg.norm(vecs, axis=1, keepdims=True) + 1e-9

    # Write centroids.f32.
    (out / "centroids.f32").write_bytes(centroids.tobytes())

    # Write cluster_<id>.bin per cluster.
    cluster_sizes = [0] * k
    cluster_files = [f"cluster_{cid:06d}.bin" for cid in range(k)]
    started = time.time()
    by_cluster: list[list[int]] = [[] for _ in range(k)]
    for doc_id in range(n):
        by_cluster[int(assignments[doc_id])].append(doc_id)
    for cid, doc_ids in enumerate(by_cluster):
        if not doc_ids:
            (out / cluster_files[cid]).write_bytes(b"\x00\x00\x00\x00")
            continue
        cluster_sizes[cid] = len(doc_ids)
        with open(out / cluster_files[cid], "wb") as f:
            f.write(struct.pack("<I", len(doc_ids)))
            for d in doc_ids:
                f.write(struct.pack("<I", d))
                f.write(vecs[d].tobytes())
        if (cid + 1) % 64 == 0:
            elapsed = time.time() - started
            print(f"  wrote cluster {cid + 1}/{k}  ({elapsed:.0f}s)", flush=True)

    # docs.jsonl — minimal metadata.
    with open(out / "docs.jsonl", "w") as f:
        for doc_id in range(n):
            obj = {
                "doc_id": doc_id,
                "uri": f"synth://mdoc/{doc_id:08d}",
                "byte_range": [0, 0],
                "snippet": f"synthetic doc #{doc_id}",
            }
            f.write(json.dumps(obj) + "\n")

    # manifest.json — matches Manifest struct + cluster_sizes vector.
    manifest = {
        "version": 1,
        "dim": dim,
        "k": k,
        "embed_model": "bge-small-en-v1.5",
        "num_docs": n,
        "centroids_file": "centroids.f32",
        "docs_file": "docs.jsonl",
        "cluster_files": cluster_files,
        "cluster_sizes": cluster_sizes,
    }
    (out / "manifest.json").write_text(json.dumps(manifest))

    print(
        f"vec: wrote {n} docs across {k} clusters "
        f"in {time.time() - started:.1f}s"
    )


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", choices=["text", "vec"], required=True)
    ap.add_argument("--n", type=int, default=1_000_000)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--shard-size", type=int, default=1000)
    ap.add_argument("--k", type=int, default=1024)
    ap.add_argument("--dim", type=int, default=384)
    ap.add_argument("--seed", type=int, default=42)
    args = ap.parse_args()

    if args.mode == "text":
        gen_text(args.out, args.n, args.shard_size, args.seed)
    else:
        gen_vec(args.out, args.n, args.k, args.dim, args.seed)
    return 0


if __name__ == "__main__":
    sys.exit(main())
