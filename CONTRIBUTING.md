# Contributing to agentic-search

Thanks for hacking on this. Short version: get the test suite green,
keep the benchmarks honest, and don't ship a regression on the
agent-loop hot path.

## Local setup

```bash
# Rust toolchain (stable, 1.78+)
rustup install stable

# pnpm 10 for the Node SDK
corepack enable && corepack use pnpm@10

# Go 1.22+ for the Go SDK
# Python 3.10+ for the Python SDKs
```

Build everything once so subsequent edits are fast:

```bash
cargo build --workspace --release
pnpm install && pnpm -r build
```

## Test loops

```bash
# Rust workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked

# Go SDK
(cd sdks/go/agenticsearch && go test ./...)

# Node SDK
(cd sdks/node/agentic-search && pnpm run build)

# Security gates (run before opening a PR)
cargo deny check
```

PRs that fail any of those won't merge — CI mirrors them exactly. Run
them locally first; round-trips on shared compute are slow.

## Benchmarks

If you touch a hot path, attach a before/after run. Numbers go in the
PR description; the JSON output goes under `bench/results/` (gitignored,
but commit the table you copy into the description).

```bash
# Set up the corpus (one-time)
python bench/macro/run.py --runs 5 --server

# Or against RustFS-on-S3
./scripts/rustfs-up.sh
python bench/macro/run.py --runs 5 --server --s3 \
    --s3-bucket agentic-search-it --s3-prefix tokio
```

The harness discards one warm-up run before measuring — please don't
"fix" that. It exists for a reason (codex round-5 P2: cold-first-run
contaminated the p50).

## What we look for in PRs

**Correctness first.** Hot-path optimisations that introduce a foot-gun
in a public API get reverted. Recent precedent: a 3 ms/file sha256
dedup was reverted because a wrong-hash caller could pollute the AST
cache. Speed isn't worth that.

**Drift safety on the search path.** Spans flow through grep → AST.
If you're emitting spans from a new producer, stamp `content_hash` on
them when downstream AST widening is expected; the server's drift
filter relies on that to drop stale spans before widening.

**No `partial_cmp` in sorts.** Every span/score sort across the
workspace uses `f32::total_cmp` plus a deterministic secondary key.
`partial_cmp` returns `None` on NaN and silently collapses to
`Equal`, which breaks tiebreakers. If you need to compare floats in
a sort, mirror the pattern in `crates/as-plan/src/lib.rs::rrf`.

**No `unwrap` / `expect` in production paths.** They're fine in
tests and bench code. Anywhere else, return a typed error.

**No new dependencies without a budget.** Adding a 200-crate
transitive tree to save 10 lines of code is a bad trade. If you
need a new dep, justify it in the PR.

**Security defaults stay restrictive.** The server binds `127.0.0.1`
by default and requires `--allow-public` to bind anywhere else.
`LocalMmapStore::safe_path` rejects `..` segments. `cargo-deny`
allowlist is documented in `deny.toml`. Don't loosen any of these
without a separate security review.

## Commit style

- Imperative tense, no trailing period on the subject.
- Subject under 72 chars.
- Body wraps at 72 chars and explains the **why**, not just the what.
- No `Co-Authored-By: Claude` lines (project policy).
- Reference the codex round / issue number when applicable.

Example:

```
fix(as-server): use f32::total_cmp in rank_search_spans for NaN safety

Aligned this sort with the heap and RRF sorts: `partial_cmp` returns
`None` on NaN and the previous fallback collapsed those to `Ordering::
Equal`, which interacts oddly with the secondary tiebreakers. ...
```

## Reporting security issues

Don't open a public issue. Use GitHub Security Advisories on this
repo — they go straight to the maintainers and stay confidential
until a fix is ready.

## License

By contributing you agree your work is licensed Apache-2.0.
