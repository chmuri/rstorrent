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
#   scripts/remote-ci.sh build --release      # remote build + copy binaries to target/release
#   scripts/remote-ci.sh fetch-bins [profile] # copy remote binaries to local target/
#   scripts/remote-ci.sh check --workspace --all-targets
#   scripts/remote-ci.sh clippy --workspace --all-targets -- -D warnings
#   scripts/remote-ci.sh fmt --check
#   scripts/remote-ci.sh test -p rt-tui --test test_download_list_scroll
#   scripts/remote-ci.sh sync                 # only rsync the working tree
#   scripts/remote-ci.sh image                # rebuild the builder image
#   scripts/remote-ci.sh doctor               # host, image and disk status
#   scripts/remote-ci.sh raw bash -lc '...'   # any command inside the container
#
# Environment overrides:
#   RSTORRENT_CI_HOST   (default: root@192.0.2.1)
#   RSTORRENT_CI_BASE   (default: /opt/rstorrent-ci)
#   RSTORRENT_CI_IMAGE  (default: rtorrent-ci:bookworm)
#   RSTORRENT_CI_JOBS   (default: auto — uses all available CPUs)
#   RSTORRENT_CI_MEMORY (default: 4g — 4 GB RAM container limit)
#
# Escape hatch for a genuinely local run: `command cargo test` (the OpenCode
# plugin only rewrites commands that start with `cargo`).
set -euo pipefail

HOST="${RSTORRENT_CI_HOST:-root@192.0.2.1}"
BASE="${RSTORRENT_CI_BASE:-/opt/rstorrent-ci}"
IMAGE="${RSTORRENT_CI_IMAGE:-rstorrent-ci:bookworm}"
JOBS="${RSTORRENT_CI_JOBS:-auto}"
MEMORY="${RSTORRENT_CI_MEMORY:-4g}"
SRC="$BASE/src"

SSH=(ssh -o BatchMode=yes -o ServerAliveInterval=30 -o ServerAliveCountMax=6 "$HOST")

usage() {
    sed -n '2,/^set /p' "$0" | sed '$d; s/^# \{0,1\}//'
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
    local cmd=(
        docker run --rm --init
        --memory="$MEMORY"
        -v "$SRC:/work"
        -v "rstorrent-target:/work/target"
        -v "rstorrent-cargo-registry:/usr/local/cargo/registry"
        -v "rstorrent-cargo-git:/usr/local/cargo/git"
        -v "$BASE/rustup:/usr/local/rustup"
    )

    # When JOBS is auto or max (default), Cargo defaults to all logical CPUs ($(nproc)).
    # If a numeric limit or other value is given, set CARGO_BUILD_JOBS explicitly.
    if [[ "$JOBS" != "auto" && "$JOBS" != "max" && -n "$JOBS" ]]; then
        cmd+=(-e "CARGO_BUILD_JOBS=$JOBS")
    fi

    cmd+=(
        -e "CARGO_TERM_COLOR=always"
        -e "CARGO_PROFILE_DEV_DEBUG=0"
        -e "CARGO_PROFILE_TEST_DEBUG=0"
        -w /work
        "$IMAGE"
    )

    local out=""
    for item in "${cmd[@]}"; do
        out+="$(printf '%q ' "$item")"
    done
    printf '%s' "$out"
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

fetch_binaries() {
    local requested_profile="${1:-}"
    local remote_mount
    remote_mount="$("${SSH[@]}" "docker volume inspect rstorrent-target --format '{{.Mountpoint}}' 2>/dev/null || echo '/var/lib/docker/volumes/rstorrent-target/_data'")"

    local profiles=()
    if [[ -n "$requested_profile" ]]; then
        profiles=("$requested_profile")
    else
        profiles=("debug" "release")
    fi

    for prof in "${profiles[@]}"; do
        local remote_dir="$remote_mount/$prof"
        local local_dir="target/$prof"
        if "${SSH[@]}" "[ -d '$remote_dir' ]"; then
            mkdir -p "$local_dir"
            echo "==> copying binaries from $HOST:$remote_dir to $local_dir" >&2
            rsync -az --include='rstorrent' --include='rstorrent-tui' --exclude='*' "$HOST:$remote_dir/" "$local_dir/" || true
        fi
    done
}

case "${1:-}" in
    -h | --help | help)
        usage
        exit 0
        ;;
    doctor)
        "${SSH[@]}" "uname -sr; echo \"CPUs: \$(nproc)\"; free -h; df -h $(printf '%q' "$BASE") | tail -1; docker images $(printf '%q' "$IMAGE") --format '{{.Repository}}:{{.Tag}} {{.Size}}'"
        echo "Builder configuration: JOBS=$JOBS, MEMORY=$MEMORY"
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
    fetch-bins | copy-bins)
        shift
        fetch_binaries "${1:-}"
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

is_build=false
build_profile="debug"
for arg in "$@"; do
    if [[ "$arg" == "build" ]]; then
        is_build=true
    elif [[ "$arg" == "--release" || "$arg" == "-r" ]]; then
        build_profile="release"
    fi
done

echo "==> cargo $* on $HOST ($IMAGE, jobs=${JOBS}, mem=${MEMORY})" >&2
run_in_container cargo "$@"
rc=$?

if [[ $rc -eq 0 && "$is_build" == "true" ]]; then
    fetch_binaries "$build_profile"
fi

exit $rc
