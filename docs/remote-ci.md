# Remote build/test runner

Compilation and tests for this repo run on a Docker builder on
`root@10.0.2.7` (Unraid), so the laptop only syncs the tree and streams output.

## Layout on the server

```
/mnt/user/appdata/rstorrent-ci/
├── Dockerfile        rust:1-bookworm + protobuf-compiler, cmake, clang, git, rsync
├── src/              working tree (rsync --delete of the repo, ~15 MB)
└── rustup/           rustup toolchains mounted at /usr/local/rustup (1.5 GB)

Docker volumes (native filesystem, NOT shfs):
  rstorrent-target            -> /work/target
  rstorrent-cargo-registry    -> /usr/local/cargo/registry
  rstorrent-cargo-git         -> /usr/local/cargo/git
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
scripts/remote-ci.sh doctor                           # host, image, disk
scripts/remote-ci.sh image                            # rebuild the builder
scripts/remote-ci.sh sync                             # only rsync
scripts/remote-ci.sh raw bash -lc 'cargo tree -d'     # anything in the container
```

Environment overrides: `RSTORRENT_CI_HOST`, `RSTORRENT_CI_BASE`,
`RSTORRENT_CI_IMAGE`, `RSTORRENT_CI_JOBS` (default `2`).

The script takes an exclusive lock (`/tmp/rstorrent-remote-ci.lock`), so
concurrent agent runs queue instead of copying the tree under each other.

## Resource limits

The host runs ~30 other containers and has 16 GB RAM, of which roughly 2–2.5 GB
is available. That is why:

- `CARGO_BUILD_JOBS` defaults to **2** (raise it only after checking `free -m`);
- `CARGO_PROFILE_DEV_DEBUG=0` and `CARGO_PROFILE_TEST_DEBUG=0` keep debug info
  out of the build, cutting disk and link time.

## OpenCode integration

`.opencode/plugins/remote-ci/index.ts` hooks `shell create.before` and rewrites
any command starting with `cargo build|check|test|clippy|fmt|nextest` to
`scripts/remote-ci.sh ...`, in every session, and raises the tool timeout to one
hour. `command cargo test` (the bash builtin) bypasses the rewrite when a local
run is genuinely wanted.

`AGENTS.md` states the same rule so the agent does not fight the hook.

## Troubleshooting

- `IMAGE_PULL`/toolchain download on first run is ~1.5 GB; it is cached in
  `/mnt/user/appdata/rstorrent-ci/rustup`.
- If `ssh` stops answering during a build, the host is thrashing: stop the
  container (`docker stop <id>`), delete the volume contents if the target is
  corrupt, and lower `RSTORRENT_CI_JOBS`.
- `rsync` excludes `.git`, `target`, `files/`, `test/`, `data/`, `dist/`,
  `*.iso`, `docs/knowledge-base`, `repomix-output.xml`, `.claude`, `.opencode`.
