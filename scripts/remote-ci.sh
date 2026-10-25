#!/usr/bin/env bash
#
# Run this repository's Rust builds and tests on the remote Docker builder.
#
# The laptop only sends the working tree (~15 MB) and streams the output; every
# compiler and test process runs on the server. Caches live under
# $BASE/{target,cargo} on the server, so repeat runs are incremental.
#
# Usage:
#   scripts/remote-ci.sh                      # cargo test --workspace
#   scripts/remote-ci.sh check --workspace --all-targets
#   scripts/remote-ci.sh clippy --workspace --all-targets -- -D warnings
#   scripts/remote-ci.sh fmt --check
#   scripts/remote-ci.sh test -p rt-tui --test test_download_list_scroll
#   scripts/remote-ci.sh sync                 # only rsync the working tree
#   scripts/remote-ci.sh image                # rebuild the builder image
#   scripts/remote-ci.sh doctor               # host, image and disk status
#   scripts/remote-ci.sh raw bash -lc '...'   # any command inside the container
#
# Escape hatch for a genuinely local run: `command cargo test` (the OpenCode
# plugin only rewrites commands that start with `cargo`).
set -euo pipefail

HOST="${RSTORRENT_CI_HOST:-root@10.0.2.7}"
BASE="${RSTORRENT_CI_BASE:-/mnt/user/appdata/rstorrent-ci}"
IMAGE="${RSTORRENT_CI_IMAGE:-rstorrent-ci:bookworm}"
JOBS="${RSTORRENT_CI_JOBS:-2}"
SRC="$BASE/src"

SSH=(ssh -o BatchMode=yes -o ServerAliveInterval=30 -o ServerAliveCountMax=6 "$HOST")

usage() {
    sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//'
}

sync_tree() {
    echo "==> syncing working tree to $HOST:$SRC" >&2
    rsync -az --delete \
        --exclude='/.git/' \
        --exclude='/target/' \
        --exclude='/files/' \
        --exclude='/data/' \
        --exclude='/test/' \
        --exclude='/test_run/' \
        --exclude='/dist/' \
        --exclude='/.claude/' \
        --exclude='/.opencode/' \
        --exclude='/.tasks/' \
        --exclude='/.task' \
        --exclude='/.ua/' \
        --exclude='/__pycache__/' \
        --exclude='/docs/knowledge-base/' \
        --exclude='/repomix-output.xml' \
        --exclude='/cookies.txt' \
        --exclude='*.iso' \
        ./ "$HOST:$SRC/"
}

docker_prefix() {
    # The target and cargo caches live in Docker volumes on the server's native
    # filesystem: Unraid's shfs (FUSE) is far too slow for a Rust build.
    printf 'docker run --rm --init -v %q -v %q -v %q -v %q -v %q -e %q -e %q -e %q -e %q -w /work %q' \
        "$SRC:/work" \
        "rstorrent-target:/work/target" \
        "rstorrent-cargo-registry:/usr/local/cargo/registry" \
        "rstorrent-cargo-git:/usr/local/cargo/git" \
        "$BASE/rustup:/usr/local/rustup" \
        "CARGO_BUILD_JOBS=$JOBS" \
        "CARGO_TERM_COLOR=always" \
        "CARGO_PROFILE_DEV_DEBUG=0" \
        "CARGO_PROFILE_TEST_DEBUG=0" \
        "$IMAGE"
}

run_in_container() {
    local quoted=""
    local arg
    for arg in "$@"; do
        quoted+=" $(printf '%q' "$arg")"
    done
    "${SSH[@]}" "mkdir -p '$SRC' '$BASE/rustup'"
    sync_tree
    "${SSH[@]}" "$(docker_prefix)${quoted}"
}

case "${1:-}" in
    -h | --help | help)
        usage
        exit 0
        ;;
    doctor)
        "${SSH[@]}" "uname -sr; nproc; df -h $(printf '%q' "$BASE") | tail -1; docker images $(printf '%q' "$IMAGE") --format '{{.Repository}}:{{.Tag}} {{.Size}}'"
        exit 0
        ;;
    image)
        "${SSH[@]}" "cd $(printf '%q' "$BASE") && docker build -t $(printf '%q' "$IMAGE") ."
        exit 0
        ;;
    sync)
        "${SSH[@]}" "mkdir -p '$SRC' '$BASE/rustup'"
        sync_tree
        exit 0
        ;;
    raw)
        shift
        [[ $# -gt 0 ]] || { echo "raw requires a command" >&2; exit 2; }
        # Serialise concurrent agent runs: one sync+build at a time.
        exec 9>"${TMPDIR:-/tmp}/rstorrent-remote-ci.lock"
        flock -w 1800 9 || { echo "another remote-ci run holds the lock" >&2; exit 75; }
        run_in_container "$@"
        exit $?
        ;;
esac

# Default: cargo, with the leading word optional.
if [[ "${1:-}" == "cargo" ]]; then
    shift
fi
if [[ $# -eq 0 ]]; then
    set -- test --workspace
fi

exec 9>"${TMPDIR:-/tmp}/rstorrent-remote-ci.lock"
flock -w 1800 9 || { echo "another remote-ci run holds the lock" >&2; exit 75; }

echo "==> cargo $* on $HOST ($IMAGE)" >&2
run_in_container cargo "$@"
