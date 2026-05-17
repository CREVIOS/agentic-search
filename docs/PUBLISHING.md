# Publishing checklist

Every tagged release (`v*`) triggers `.github/workflows/release.yml`,
which builds + uploads:

- CLI binaries (linux/darwin/windows × amd64+arm64) → GitHub Release
- Python sdists + wheels for all 5 SDK adapters → GitHub Release + PyPI
- Node tarball (`@agentic-search/sdk`) → GitHub Release + npm
- Rust workspace crates (11) → crates.io
- Multi-arch container image → GHCR

Registry publishing is **gated on secrets** so the workflow still runs
cleanly on a fresh repo. Add the secrets below and tag `v0.x.y` —
everything ships automatically.

## One-time setup

### 1. PyPI (5 packages)

Each Python SDK is a separate PyPI project. **Names must be claimed
first** — anyone can squat a name. Register:

- [claude-agent-search](https://pypi.org/project/claude-agent-search/)
- [openai-agentic-search](https://pypi.org/project/openai-agentic-search/)
- [deepagents-search](https://pypi.org/project/deepagents-search/)
- [langchain-agentic-search](https://pypi.org/project/langchain-agentic-search/)
- [crewai-agentic-search](https://pypi.org/project/crewai-agentic-search/)

Recommended path: **trusted publishing via GitHub OIDC** (no token
needed). For each project on PyPI:

1. Create project (push a 0.0.0 sdist manually once if PyPI requires
   an existing project) or use PyPI's "Add a pending publisher"
   feature.
2. Settings → Publishing → "Add a new pending publisher":
    - Owner: `CREVIOS`
    - Repository: `agentic-search`
    - Workflow: `release.yml`
    - Environment: leave empty

The `pypa/gh-action-pypi-publish@release/v1` step in `release.yml`
already authenticates via `id-token: write`. No `PYPI_API_TOKEN` secret
needed once each project is wired up.

**Fallback (classic token):** set `PYPI_API_TOKEN` repo secret to a
scoped PyPI token. The workflow falls back to twine + token upload.

### 2. npm (`@agentic-search/sdk`)

1. Create the `@agentic-search` organisation on [npmjs.com](https://www.npmjs.com/org/create).
2. Generate an **automation** token: `npm token create --type=automation`.
3. Add as repo secret `NPM_TOKEN`.

`package.json` already has `publishConfig.access: "public"` so the
first publish on a scoped package works without flags.

### 3. crates.io (11 workspace crates)

1. Create a crates.io API token at [crates.io/me](https://crates.io/me).
2. Add as repo secret `CARGO_REGISTRY_TOKEN`.

First-time owners need to **claim** each crate name. The 11
crates publish under the `agentic-search-*` prefix on crates.io
(the bare `as-*` names were squatted). The in-tree Rust imports
(`use as_core::*`, etc.) keep working because each `Cargo.toml`
declares `[lib] name = "as_*"`. Dependency-order publish:

```
agentic-search-core      →  agentic-search-embed  →
agentic-search-store     →  agentic-search-fs     →
agentic-search-cache     →  agentic-search-grep   →
agentic-search-ast       →  agentic-search-vec    →
agentic-search-plan      →  agentic-search-server →
agentic-search-cli
```

Each step tolerates "already published at this version" so re-running
a partial release is safe.

### 4. GHCR (Docker image)

Zero setup — uses the workflow's `GITHUB_TOKEN` and `packages: write`
permission. Image lands at
`ghcr.io/crevios/agentic-search:<version>`.

## Cutting a release

```bash
# 1. Update version in workspace Cargo.toml + pyproject.toml(s) + package.json.
#    Example: bump 0.1.0 → 0.1.1
sed -i '' 's/0.1.0/0.1.1/' Cargo.toml \
  sdks/python/*/pyproject.toml \
  sdks/node/agentic-search/package.json

# 2. Update CHANGELOG.md with the new entry.

# 3. Commit + tag.
git add -A
git commit -m "release: v0.1.1"
git tag v0.1.1
git push origin main v0.1.1
```

The tag push fires `release.yml`. Track progress at
https://github.com/CREVIOS/agentic-search/actions.

## Manual fallback (if CI is down)

If you need to publish bypassing CI (rare; only when CI is dead):

```bash
# PyPI — one per package
for pkg in sdks/python/*/; do
  python -m build --sdist --wheel "$pkg" --outdir /tmp/dist-py
done
twine upload /tmp/dist-py/*

# npm
cd sdks/node/agentic-search
pnpm run build
npm publish --access public

# crates.io — strict order. -p takes the crates.io name.
for crate in \
  agentic-search-core agentic-search-embed agentic-search-store \
  agentic-search-fs agentic-search-cache agentic-search-grep \
  agentic-search-ast agentic-search-vec agentic-search-plan \
  agentic-search-server agentic-search-cli ; do
  cargo publish -p "$crate" --locked --no-verify
  sleep 30
done

# GHCR
docker buildx build --platform linux/amd64,linux/arm64 \
  -t ghcr.io/crevios/agentic-search:0.1.1 \
  -t ghcr.io/crevios/agentic-search:latest \
  --push .
```

## Verifying a release

```bash
# CLI
cargo install --version 0.1.1 agentic-search-cli

# Python (one example)
pip install --upgrade claude-agent-search==0.1.1

# Node
npm view @agentic-search/sdk@0.1.1

# Docker
docker pull ghcr.io/crevios/agentic-search:0.1.1
docker run --rm ghcr.io/crevios/agentic-search:0.1.1 --version
```
