# Multi-stage build for the agentic-search server.
#
# Builder pulls the full Rust toolchain and produces a release binary;
# the runtime stage is a slim debian image with just the binary, the
# fastembed ONNX runtime download dir, and tini for clean signal
# handling. Image size hovers around 120 MB once the ONNX runtime
# binary is in place.

ARG RUST_VERSION=1.86
FROM rust:${RUST_VERSION}-slim-bookworm AS builder

ARG TARGETPLATFORM
WORKDIR /src

# System deps fastembed/ort + tree-sitter grammars need at build time.
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        ca-certificates \
        clang \
        cmake \
        git \
    && rm -rf /var/lib/apt/lists/*

# Cache deps separately from sources so a source-only edit doesn't
# invalidate the dependency layer.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY bench ./bench
# Pre-fetch + build all deps for the binary we ship.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p as-cli && \
    cp target/release/agentic-search /usr/local/bin/agentic-search

FROM debian:bookworm-slim AS runtime

# tini handles PID 1 zombies; ca-certificates lets reqwest hit HTTPS
# (Brave/Tavily/Exa, S3 with rustls).
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        tini \
        libgcc-s1 \
    && rm -rf /var/lib/apt/lists/*

# Run as a non-root user. The cache and embedder model dirs live under
# /var/lib/agentic-search and are writable by that user.
RUN useradd --create-home --home-dir /var/lib/agentic-search --shell /usr/sbin/nologin --uid 10001 agentic
USER agentic
WORKDIR /var/lib/agentic-search
ENV FASTEMBED_CACHE_PATH=/var/lib/agentic-search/.fastembed_cache \
    RUST_LOG=info

COPY --from=builder /usr/local/bin/agentic-search /usr/local/bin/agentic-search

EXPOSE 8787

# Default to REST on 0.0.0.0:8787 *with* --allow-public so the
# container is actually reachable. The CLI's loopback default is right
# for laptop dev but wrong for a container — the container boundary
# already enforces "non-host" reachability. Run a reverse proxy in
# front for auth.
ENTRYPOINT ["/usr/bin/tini", "--", "agentic-search"]
CMD ["serve", "--bind", "0.0.0.0:8787", "--allow-public"]
