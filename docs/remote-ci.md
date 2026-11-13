# Remote build/test runner

Compilation and tests for this repo run on a Docker builder on
`root@builder.example.com` (Unraid), so the laptop only syncs the tree and streams output.

## Layout on the server

```
/opt/rstorrent-ci/
├── Dockerfile        rust:1-bookworm + protobuf-compiler, cmake, clang, git, rsync, sccache
├── src/              working tree (rsync --delete of the repo, ~15 MB)
└── rustup/           rustup toolchains mounted at /usr/local/rustup (1.5 GB)

Docker volumes (native filesystem, NOT shfs):
  rstorrent-target            -> /work/target
  rstorrent-cargo-registry    -> /usr/local/cargo/registry
  rstorrent-cargo-git         -> /usr/local/cargo/git

Shared S3 Build Cache:
  RustFS on root@builder.example.com:9000 (bucket: `sccache`)
  Used by both the remote builder and local development builds
```

`target/` **must** stay on a Docker volume. Unraid's `shfs` (FUSE) is a single
bottleneck and turns this build into an I/O storm; the volumes live on
`/var/lib/docker` (ext4 loop device).

## Usage

```bash
scripts/remote-ci.sh                                  # cargo test --workspace
scripts/remote-ci.sh check --workspace --all-targets
scripts/remote-ci.sh clippy --workspace --all-targets -- -D warnings
scripts/remote-ci.sh fmt --check
scripts/remote-ci.sh test -p rt-core --test test_view
scripts/remote-ci.sh doctor                           # host, image, disk, rustfs status
scripts/remote-ci.sh image                            # rebuild the builder
scripts/remote-ci.sh sccache                          # show sccache stats in container
scripts/remote-ci.sh sync                             # only rsync
scripts/remote-ci.sh raw bash -lc 'cargo tree -d'     # anything in the container
```

Environment overrides: `RSTORRENT_CI_HOST`, `RSTORRENT_CI_BASE`,
`RSTORRENT_CI_IMAGE`, `RSTORRENT_CI_JOBS` (default `auto` — uses all available CPUs),
`RSTORRENT_CI_MEMORY` (default `6g` — limits Docker container to 6 GB RAM).

The script takes an exclusive lock (`/tmp/rstorrent-remote-ci.lock`), so
concurrent agent runs queue instead of copying the tree under each other.

## Resource limits

The remote server runs other background containers and has 16 GB physical RAM.
To maximize build performance while preserving host stability:

- **Memory limit**: Container RAM is capped at **6 GB** (`--memory=6g` /
  `RSTORRENT_CI_MEMORY`), preventing cargo/rustc memory spikes from impacting host services;
- **CPU parallelism**: `RSTORRENT_CI_JOBS` defaults to **auto** (all logical
  cores on the Xeon host, e.g. 36 threads), maximizing compilation speed across crates;
- `CARGO_PROFILE_DEV_DEBUG=0` and `CARGO_PROFILE_TEST_DEBUG=0` keep debug info
  out of the build, cutting disk and link time.

## Laptop shell integration (prevent local CPU burning)

By default, typing `cargo ...` directly in a terminal on the laptop executes locally,
consuming all laptop CPU cores. To seamlessly route `cargo` calls inside this
repository to the remote builder, add this wrapper function to your `~/.bashrc`:

```bash
cargo() {
    if [[ -f "./scripts/remote-ci.sh" && -d ".git" ]]; then
        ./scripts/remote-ci.sh cargo "$@"
    else
        command cargo "$@"
    fi
}
```

Or invoke the wrapper directly:
```bash
./scripts/remote-ci.sh check --workspace --all-targets
./scripts/remote-ci.sh test -p rt-tui
./scripts/remote-ci.sh build --release
```

## Shared S3 Cache (sccache & RustFS)

The setup uses `sccache` backed by RustFS (high-performance S3-compatible object storage) running on `builder.example.com:9000`:
- **Bucket**: `sccache`
- **Config**: `.cargo/config.toml` configures `rustc-wrapper = "sccache"`, sets `[profile.dev] incremental = false`, and injects S3 endpoint/credentials into compiler processes.
- **Local config**: Local user configuration is also stored in `~/.config/sccache/config` and `~/.aws/credentials`.
- **Remote CI**: The remote container image includes `sccache` with S3 support enabled and connects to `http://builder.example.com:9000`. Cache statistics are reported after remote cargo runs.

## Local agent tooling (optional, not part of the repository)

A local-only OpenCode plugin can hook `shell create.before` and rewrite any
command starting with `cargo build|check|test|clippy|fmt|nextest` to
`scripts/remote-ci.sh ...`, raising the tool timeout to one hour. It is personal
agent tooling kept outside the repository; `command cargo test` (the bash
builtin) bypasses the rewrite when a local run is genuinely wanted.

`AGENTS.md` states the same rule so the agent does not fight the hook.

## Troubleshooting

- `IMAGE_PULL`/toolchain download on first run is ~1.5 GB; it is cached in
  `/opt/rstorrent-ci/rustup`.
- If `ssh` stops answering during a build, the host is thrashing: stop the
  container (`docker stop <id>`), delete the volume contents if the target is
  corrupt, and lower `RSTORRENT_CI_JOBS`.
- `rsync` excludes `.git`, `target`, `files/`, `test/`, `data/`, `dist/`,
  `*.iso`, `docs/knowledge-base`, `repomix-output.xml`, `.claude`, `.opencode`.
