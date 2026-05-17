#!/usr/bin/env bash
# Build a real ~1 GB / 10k+ markdown corpus for the examples.
# Sources all public, CC-BY-SA / MIT / Apache-licensed docs.
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

# 1. The Rust Book — ~20 chapters of substantive technical prose.
clone_sparse https://github.com/rust-lang/book main src "$OUT/rust-book"

# 2. Kubernetes concepts — huge, complex multi-doc reference.
clone_sparse https://github.com/kubernetes/website main \
  content/en/docs/concepts "$OUT/k8s-concepts"

# 3. Tokio tutorial — async runtime walkthrough.
clone_sparse https://github.com/tokio-rs/website master \
  content/tokio/tutorial "$OUT/tokio-tutorial"

# 4. MDN — web/javascript reference. Dominant source by file count.
clone_sparse https://github.com/mdn/content main \
  files/en-us/web/javascript "$OUT/mdn-javascript"

# 5. MDN — web/api. Another large block.
clone_sparse https://github.com/mdn/content main \
  files/en-us/web/api "$OUT/mdn-webapi"

# 6. MDN — web/css for a third major section.
clone_sparse https://github.com/mdn/content main \
  files/en-us/web/css "$OUT/mdn-css"

du -sh "$OUT"/*  "$OUT"
echo "files: $(find "$OUT" -name '*.md' | wc -l | tr -d ' ')"
