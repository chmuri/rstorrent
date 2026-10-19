# syntax=docker/dockerfile:1

# Stage 1: Build the Rust rstorrent binary
FROM rust:bookworm AS builder

WORKDIR /build

# rt-grpc's build script compiles the protobuf contract at build time.
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

# Copy project workspace definition and source code
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/

# Build the release binary for rstorrent
RUN cargo build --release -p rstorrent

# Stage 2: Minimal runtime image
FROM debian:bookworm-slim

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
