# Agent notes

## Builds and tests run on the remote builder

This project compiles and tests on a Docker builder on `root@10.0.2.7` so the
laptop stays free. The wrapper is `scripts/remote-ci.sh`; the builder lives at
`/mnt/user/appdata/rstorrent-ci` (image `rstorrent-ci:bookworm`, cargo and target
caches in sibling directories).

Rules:

- **Never run `cargo build`, `cargo check`, `cargo test`, `cargo clippy` or
  `cargo fmt` locally.** Use the wrapper:
  - `scripts/remote-ci.sh` — `cargo test --workspace`
  - `scripts/remote-ci.sh check --workspace --all-targets`
  - `scripts/remote-ci.sh clippy --workspace --all-targets -- -D warnings`
  - `scripts/remote-ci.sh fmt --check`
  - `scripts/remote-ci.sh test -p rt-core --test test_view`
- The plugin `.opencode/plugins/remote-ci/index.ts` rewrites bare `cargo ...`
  shell calls to the wrapper automatically, in every session.
- `command cargo ...` is the escape hatch for an intentional local run.
- `scripts/remote-ci.sh doctor` reports builder health; `image` rebuilds the
  builder; `sync` only copies the working tree; `raw <cmd...>` runs any command
  inside the container.
- A run is serialised with a lock: concurrent agents wait instead of copying the
  tree under each other.
- The builder excludes `.git`, `target`, `files/`, `test/`, `dist/`, `*.iso`,
  `docs/knowledge-base`, `repomix-output.xml` and `.opencode`; only source,
  crates, scripts and docs are sent (~15 MB).

## Local work

Reading, editing, formatting decisions and benchmarks that drive the local
binary stay local. The remote builder is only for compile/test feedback.
