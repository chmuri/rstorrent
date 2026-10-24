#!/usr/bin/env bash
# Run the Phase 0 benchmark suite against a synthetic session.
#
# Writes machine-readable results into docs/benchmarks/<timestamp>/ and keeps
# the (large) generated session under $TMPDIR/rstorrent-bench/.
#
# Usage:
#   scripts/bench/run_all.sh                          # 14k startup + lookup + multicall
#   scripts/bench/run_all.sh --count 1000 --no-build
#   scripts/bench/run_all.sh --recheck-count 100 --recheck-mb 32
set -euo pipefail

cd "$(dirname "$0")/../.."
REPO_ROOT=$PWD
BIN="$REPO_ROOT/target/release/rstorrent"
COUNT=14000
LOOKUPS=10000
MULTICALL=20
RECHECK_COUNT=0
RECHECK_MB=32
NO_BUILD=0
STAMP=$(date +%Y%m%dT%H%M%S)
OUT=${OUT:-"$REPO_ROOT/docs/benchmarks/$STAMP"}
WORK=${WORK:-"${TMPDIR:-/tmp/opencode}/rstorrent-bench/$STAMP"}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --count) COUNT=$2; shift 2 ;;
        --lookups) LOOKUPS=$2; shift 2 ;;
        --multicall) MULTICALL=$2; shift 2 ;;
        --recheck-count) RECHECK_COUNT=$2; shift 2 ;;
        --recheck-mb) RECHECK_MB=$2; shift 2 ;;
        --out) OUT=$2; shift 2 ;;
        --work) WORK=$2; shift 2 ;;
        --bin) BIN=$2; shift 2 ;;
        --no-build) NO_BUILD=1; shift ;;
        -h|--help) sed -n '2,12p' "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

mkdir -p "$OUT" "$WORK"
PID=""
stop_daemon() {
    [[ -n "${PID:-}" ]] || return 0
    if kill -0 "$PID" 2>/dev/null; then
        kill -TERM "$PID" 2>/dev/null || true
        for _ in $(seq 1 200); do
            kill -0 "$PID" 2>/dev/null || break
            sleep 0.05
        done
        kill -KILL "$PID" 2>/dev/null || true
    fi
    PID=""
}
cleanup() { stop_daemon; }
trap cleanup EXIT

if [[ $NO_BUILD -eq 0 ]]; then
    cargo build --release -p rstorrent
fi

{
    uname -a
    echo "nproc: $(nproc 2>/dev/null || echo unknown)"
    echo "nofile: $(ulimit -n)"
    echo "work fs: $(stat -f -c '%T' "$WORK" 2>/dev/null || echo unknown)"
    "$BIN" --version
    python3 --version
} > "$OUT/host.txt" 2>&1

echo "==> startup: $COUNT torrents"
python3 scripts/bench/gen_session.py --out "$WORK/session" --count "$COUNT"
python3 scripts/bench/bench_start.py \
    --session "$WORK/session" \
    --data "$WORK/data" \
    --expected "$COUNT" \
    --bin "$BIN" \
    --keep-running \
    --json-out "$OUT/startup.json"
PID=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$OUT/startup.json")

echo "==> lookup: $LOOKUPS calls"
python3 scripts/bench/bench_lookup.py \
    --sock "$WORK/session/bench.sock" \
    --manifest "$WORK/session/manifest.json" \
    --iterations "$LOOKUPS" \
    --json-out "$OUT/lookup.json"

echo "==> multicall2: $MULTICALL iterations"
python3 scripts/bench/bench_multicall.py \
    --sock "$WORK/session/bench.sock" \
    --iterations "$MULTICALL" \
    --json-out "$OUT/multicall.json"

if [[ "$RECHECK_COUNT" -gt 0 ]]; then
    echo "==> recheck: $RECHECK_COUNT x ${RECHECK_MB} MiB"
    stop_daemon

    python3 scripts/bench/gen_payload.py \
        --out "$WORK/payload/session" \
        --data "$WORK/payload/data" \
        --count "$RECHECK_COUNT" \
        --size-mb "$RECHECK_MB"

    python3 scripts/bench/bench_start.py \
        --session "$WORK/payload/session" \
        --data "$WORK/payload/data" \
        --sock "$WORK/payload/session/bench.sock" \
        --expected "$RECHECK_COUNT" \
        --bin "$BIN" \
        --keep-running \
        --json-out "$OUT/recheck-startup.json"
    PID=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$OUT/recheck-startup.json")

    python3 scripts/bench/bench_recheck.py \
        --sock "$WORK/payload/session/bench.sock" \
        --pid "$PID" \
        --manifest "$WORK/payload/session/payload-manifest.json" \
        --count "$RECHECK_COUNT" \
        --json-out "$OUT/recheck.json"
fi

echo
echo "results: $OUT"
python3 - "$OUT" <<'PY'
import json
import os
import sys

out = sys.argv[1]
for name in ("startup", "lookup", "multicall", "recheck"):
    path = os.path.join(out, f"{name}.json")
    if not os.path.exists(path):
        continue
    data = json.load(open(path))
    keys = (
        "full_s", "rows", "peak",
        "p50_ms", "p95_ms", "p99_ms",
        "wall_s", "mb_per_s", "peak_active_checks", "peak_threads",
    )
    summary = {key: data[key] for key in keys if key in data}
    print(f"{name:10} {json.dumps(summary)}")
PY
