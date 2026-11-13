FROM rust:1-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
        protobuf-compiler pkg-config cmake clang libclang-dev \
        git rsync ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

RUN curl -fsSL https://github.com/mozilla/sccache/releases/download/v0.19.0/sccache-v0.19.0-x86_64-unknown-linux-musl.tar.gz \
    | tar -xz -C /tmp && \
    mv /tmp/sccache-v0.19.0-x86_64-unknown-linux-musl/sccache /usr/local/bin/sccache && \
    chmod +x /usr/local/bin/sccache && \
    rm -rf /tmp/sccache*

# sccache S3 credentials are injected at build time via --build-arg (see
# scripts/remote-ci.sh `image`); no secrets are stored in this repository.
ARG SCCACHE_ENDPOINT=http://cache.example.com:9000
ARG AWS_ACCESS_KEY_ID=""
ARG AWS_SECRET_ACCESS_KEY=""

ENV CARGO_TERM_COLOR=always \
    CARGO_INCREMENTAL=0 \
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
    PROTOC=/usr/bin/protoc \
    RUSTUP_TOOLCHAIN=stable \
    RUSTC_WRAPPER=sccache \
    SCCACHE_BUCKET=sccache \
    SCCACHE_ENDPOINT=${SCCACHE_ENDPOINT} \
    SCCACHE_S3_USE_SSL=false \
    AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID} \
    AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY} \
    SCCACHE_REGION=us-east-1

WORKDIR /work
