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

ENV CARGO_TERM_COLOR=always \
    CARGO_INCREMENTAL=0 \
    CARGO_NET_GIT_FETCH_WITH_CLI=true \
    PROTOC=/usr/bin/protoc \
    RUSTUP_TOOLCHAIN=stable \
    RUSTC_WRAPPER=sccache \
    SCCACHE_BUCKET=sccache \
    SCCACHE_ENDPOINT=http://hades.chmuri.net:9000 \
    SCCACHE_S3_USE_SSL=false \
    AWS_ACCESS_KEY_ID=AAXRZ9NNKXBW76H9X5NP \
    AWS_SECRET_ACCESS_KEY=95ef3340e7b22d535fa62fe10551559aebd74a58d2c212a7 \
    SCCACHE_REGION=us-east-1

WORKDIR /work
