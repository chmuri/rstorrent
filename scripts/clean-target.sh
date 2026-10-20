#!/usr/bin/env bash
# scripts/clean-target.sh: Automatically caps and trims Cargo build artifacts in target/
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$WORKSPACE_ROOT"

MAX_SIZE_GB="${MAX_SIZE_GB:-4}"
TARGET_DIR="${WORKSPACE_ROOT}/target"

if [ ! -d "$TARGET_DIR" ]; then
    exit 0
fi

CURRENT_SIZE_KB=$(du -sk "$TARGET_DIR" | cut -f1)
MAX_SIZE_KB=$(( MAX_SIZE_GB * 1024 * 1024 ))

echo "[clean-target] Checking target directory size: $(( CURRENT_SIZE_KB / 1024 )) MB (Limit: ${MAX_SIZE_GB} GB)"

if [ "$CURRENT_SIZE_KB" -gt "$MAX_SIZE_KB" ] || [ "${1:-}" = "--force" ]; then
    echo "[clean-target] Target exceeds limit or force specified. Sweeping old artifacts..."
    
    if command -v cargo-sweep >/dev/null 2>&1; then
        cargo-sweep sweep --maxsize "${MAX_SIZE_GB}GB" || true
    fi

    # Clean old incremental compilation sessions older than 2 days
    if [ -d "$TARGET_DIR/debug/incremental" ]; then
        find "$TARGET_DIR/debug/incremental" -mindepth 1 -maxdepth 1 -mtime +2 -exec rm -rf {} + 2>/dev/null || true
    fi

    NEW_SIZE_KB=$(du -sk "$TARGET_DIR" | cut -f1)
    echo "[clean-target] Cleanup complete. New size: $(( NEW_SIZE_KB / 1024 )) MB (Freed: $(( (CURRENT_SIZE_KB - NEW_SIZE_KB) / 1024 )) MB)"
else
    echo "[clean-target] Target directory size is within limits. No cleanup needed."
fi
