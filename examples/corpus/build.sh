#!/usr/bin/env bash
# Build a realistic ~30 MB markdown corpus for the examples.
# Sources are all public, MIT/Apache/CC-BY-SA-licensed docs.
#
# Output: examples/corpus/data/<source>/**/*.md
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
OUT="$ROOT/data"
rm -rf "$OUT"
mkdir -p "$OUT"

clone_sparse() {
  local url="$1"  ref="$2"  pattern="$3"  outdir="$4"
  echo "== $url@$ref → $outdir =="
  rm -rf /tmp/.corpus-clone
  git clone --depth=1 --filter=blob:none --sparse --branch="$ref" "$url" /tmp/.corpus-clone >/dev/null
  (cd /tmp/.corpus-clone && git sparse-checkout set "$pattern" >/dev/null)
  mkdir -p "$outdir"
  rsync -a --include='*/' --include='*.md' --exclude='*' /tmp/.corpus-clone/$pattern/ "$outdir/"
  rm -rf /tmp/.corpus-clone
}

# 1. The Rust Book (~20 chapters, substantive technical prose).
clone_sparse https://github.com/rust-lang/book main src "$OUT/rust-book"

# 2. Kubernetes concepts (huge, complex multi-doc reference).
clone_sparse https://github.com/kubernetes/website main \
  content/en/docs/concepts "$OUT/k8s-concepts"

# 3. Tokio tutorial (async runtime walkthrough).
clone_sparse https://github.com/tokio-rs/website master \
  content/tokio/tutorial "$OUT/tokio-tutorial"

du -sh "$OUT"/*  "$OUT"
echo "files: $(find "$OUT" -name '*.md' | wc -l | tr -d ' ')"
