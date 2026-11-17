# syntax=docker/dockerfile:1

# Stage 1: Build the Rust rstorrent binary
FROM rust:bookworm AS builder

WORKDIR /build

# rt-grpc's build script compiles the protobuf contract at build time.
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Install Trunk (WASM UI bundler) and sccache (compiler cache) before copying
# sources so this layer is reused across dependency bumps. Use static musl
# builds: the gnu prebuilts require GLIBC_2.39, newer than rust:bookworm's 2.36.
RUN case "$(uname -m)" in \
         x86_64) trunk_asset=trunk-x86_64-unknown-linux-musl.tar.gz; sccache_target=x86_64-unknown-linux-musl ;; \
         aarch64) trunk_asset=trunk-aarch64-unknown-linux-musl.tar.gz; sccache_target=aarch64-unknown-linux-musl ;; \
       esac \
    && curl -fsSL "https://github.com/trunk-rs/trunk/releases/download/v0.22.1/$trunk_asset" \
       | tar xz -C /usr/local/bin \
    && curl -fsSL "https://github.com/mozilla/sccache/releases/download/v0.19.0/sccache-v0.19.0-$sccache_target.tar.gz" \
       | tar xz -C /usr/local/bin --strip-components=1 --wildcards '*/sccache' \
    && rustup target add wasm32-unknown-unknown

# sccache caches individual rustc invocations by content hash, so bumping one
# crate recompiles only it and its dependents. In CI the cache is backed by the
# GitHub Actions cache (credentials mounted as secrets in the build steps below);
# without them sccache falls back to its local on-disk cache.
ENV RUSTC_WRAPPER=sccache

# Copy project workspace definition and source code (ui/ is a workspace member)
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY ui/ ./ui/

# Build the Leptos WebAssembly UI (ui/dist) that build.rs embeds into the binary.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=secret,id=ACTIONS_RESULTS_URL \
    --mount=type=secret,id=ACTIONS_RUNTIME_TOKEN \
    if [ -f /run/secrets/ACTIONS_RUNTIME_TOKEN ]; then \
      export ACTIONS_RESULTS_URL="$(cat /run/secrets/ACTIONS_RESULTS_URL)" \
             ACTIONS_RUNTIME_TOKEN="$(cat /run/secrets/ACTIONS_RUNTIME_TOKEN)" \
             SCCACHE_GHA_ENABLED=on; \
    fi \
    && cd ui && trunk build --release

# Build the release binary for rstorrent
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=secret,id=ACTIONS_RESULTS_URL \
    --mount=type=secret,id=ACTIONS_RUNTIME_TOKEN \
    if [ -f /run/secrets/ACTIONS_RUNTIME_TOKEN ]; then \
      export ACTIONS_RESULTS_URL="$(cat /run/secrets/ACTIONS_RESULTS_URL)" \
             ACTIONS_RUNTIME_TOKEN="$(cat /run/secrets/ACTIONS_RUNTIME_TOKEN)" \
             SCCACHE_GHA_ENABLED=on; \
    fi \
    && cargo build --release -p rstorrent

# Stage 2: Minimal runtime image
FROM debian:bookworm-slim

LABEL org.opencontainers.image.title="rstorrent" \
      org.opencontainers.image.description="Fast, modern BitTorrent client in Rust with rTorrent CLI & Flood web UI compatibility" \
      org.opencontainers.image.url="https://github.com/chmuri/rstorrent" \
      org.opencontainers.image.source="https://github.com/chmuri/rstorrent" \
      org.opencontainers.image.licenses="GPL-2.0-or-later"

# Install runtime dependencies: SSL certificates, curl, and libc
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create application directories for persistent volumes
RUN mkdir -p /downloads /session /config /etc/rstorrent /etc/rtorrent

# Copy compiled binary from builder and create rtorrent compatibility symlink
COPY --from=builder /build/target/release/rstorrent /usr/local/bin/rstorrent
RUN ln -sf /usr/local/bin/rstorrent /usr/local/bin/rtorrent

# Copy default configuration templates and entrypoint script
COPY rstorrent.default.rc /etc/rstorrent/rstorrent.default.rc
RUN ln -sf /etc/rstorrent/rstorrent.default.rc /etc/rtorrent/rtorrent.default.rc
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

# Expose network ports:
# 5000: SCGI XML-RPC / JSON-RPC server (Flood Web UI)
# 50000: BitTorrent incoming P2P connections
# 6881/udp: DHT (Distributed Hash Table)
EXPOSE 5000 50000 6881/udp

# Volumes for downloads, session resume data, and configuration
VOLUME ["/downloads", "/session", "/config"]

# Start entrypoint to initialize config on empty mounts
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["/usr/local/bin/rstorrent", "--daemon", "-c", "/config/.rstorrent.rc"]
