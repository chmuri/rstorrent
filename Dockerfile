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
    && curl --proto '=https' --tlsv1.2 -fsSL "https://github.com/trunk-rs/trunk/releases/download/v0.22.1/$trunk_asset" \
       | tar xz -C /usr/local/bin \
    && curl --proto '=https' --tlsv1.2 -fsSL "https://github.com/mozilla/sccache/releases/download/v0.19.0/sccache-v0.19.0-$sccache_target.tar.gz" \
       | tar xz -C /usr/local/bin --strip-components=1 --wildcards '*/sccache' \
    && rustup target add wasm32-unknown-unknown

# sccache caches individual rustc invocations by content hash, so bumping one
# crate recompiles only it and its dependents. It is enabled per build step on
# amd64 only: the sccache server fails to start under QEMU arm64 emulation
# ("failed to fill whole buffer"), so arm64 builds use plain rustc. In CI the
# cache is backed by the GitHub Actions cache (credentials mounted as secrets in
# the build steps below); without them sccache falls back to its on-disk cache.
# ACTIONS_CACHE_SERVICE_V2 must also reach the build container: opendal picks the
# cache protocol (V1 legacy REST vs V2 twirp) from that flag, and the runner's
# results-receiver endpoint only speaks V2. Without it sccache silently fails
# every cache operation and falls back to disk.

# No shared cargo-registry cache mount: a single mount is shared across the
# amd64/arm64 builds, so concurrent cargo unpacks race on .cargo-ok (EEXIST).
# Downloads carry from the trunk step to the cargo step via per-platform layers.

# Copy project workspace definition and source code (ui/ is a workspace member)
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY ui/ ./ui/

# Build the Leptos WebAssembly UI (ui/dist) that build.rs embeds into the binary.
WORKDIR /build/ui
RUN --mount=type=secret,id=ACTIONS_RESULTS_URL \
    --mount=type=secret,id=ACTIONS_RUNTIME_TOKEN \
    --mount=type=secret,id=ACTIONS_CACHE_SERVICE_V2 \
    if [ "$(uname -m)" = x86_64 ] && [ -f /run/secrets/ACTIONS_RUNTIME_TOKEN ]; then \
      export RUSTC_WRAPPER=sccache \
             ACTIONS_RESULTS_URL="$(cat /run/secrets/ACTIONS_RESULTS_URL)" \
             ACTIONS_RUNTIME_TOKEN="$(cat /run/secrets/ACTIONS_RUNTIME_TOKEN)" \
             ACTIONS_CACHE_SERVICE_V2="$(cat /run/secrets/ACTIONS_CACHE_SERVICE_V2 2>/dev/null)" \
             SCCACHE_GHA_ENABLED=on; \
    fi \
    && trunk build --release

# Build the release binary for rstorrent
WORKDIR /build
RUN --mount=type=secret,id=ACTIONS_RESULTS_URL \
    --mount=type=secret,id=ACTIONS_RUNTIME_TOKEN \
    --mount=type=secret,id=ACTIONS_CACHE_SERVICE_V2 \
    if [ "$(uname -m)" = x86_64 ] && [ -f /run/secrets/ACTIONS_RUNTIME_TOKEN ]; then \
      export RUSTC_WRAPPER=sccache \
             ACTIONS_RESULTS_URL="$(cat /run/secrets/ACTIONS_RESULTS_URL)" \
             ACTIONS_RUNTIME_TOKEN="$(cat /run/secrets/ACTIONS_RUNTIME_TOKEN)" \
             ACTIONS_CACHE_SERVICE_V2="$(cat /run/secrets/ACTIONS_CACHE_SERVICE_V2 2>/dev/null)" \
             SCCACHE_GHA_ENABLED=on; \
    fi \
    && cargo build --release --locked -p rstorrent \
    && if [ "$(uname -m)" = x86_64 ] && [ -f /run/secrets/ACTIONS_RUNTIME_TOKEN ]; then sccache --show-stats; fi

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
RUN mkdir -p /downloads /session /config /watch /etc/rstorrent /etc/rtorrent

# Copy compiled binary from builder and create rtorrent compatibility symlink
COPY --from=builder /build/target/release/rstorrent /usr/local/bin/rstorrent
RUN ln -sf /usr/local/bin/rstorrent /usr/local/bin/rtorrent

# Copy default configuration templates and entrypoint script
COPY rstorrent.default.rc /etc/rstorrent/rstorrent.default.rc
RUN ln -sf /etc/rstorrent/rstorrent.default.rc /etc/rtorrent/rtorrent.default.rc
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

# Expose network ports:
# 5000: SCGI XML-RPC / JSON-RPC server
# 8080: Native Web UI
# 50000: BitTorrent incoming P2P connections
# 6881/udp: DHT (Distributed Hash Table)
EXPOSE 5000 8080 50000 6881/udp

# Volumes for downloads, session resume data, and configuration
VOLUME ["/downloads", "/session", "/config", "/watch"]

# Start entrypoint to initialize config on empty mounts
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["/usr/local/bin/rstorrent", "--daemon", "-c", "/config/.rstorrent.rc", "--webui-addr", "0.0.0.0:8080", "--watch-dir", "/watch"]
