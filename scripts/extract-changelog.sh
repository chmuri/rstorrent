#!/usr/bin/env bash
#
# Prints the changelog section for a release tag.
# Usage: scripts/extract-changelog.sh v0.10.0 [CHANGELOG.md]
set -euo pipefail

RAW_VERSION="${1:?usage: extract-changelog.sh <version|vversion> [CHANGELOG.md]}"
VERSION="${RAW_VERSION#v}"
CHANGELOG="${2:-CHANGELOG.md}"

if [ ! -f "${CHANGELOG}" ]; then
    echo "Missing ${CHANGELOG}" >&2
    exit 1
fi

awk -v version="${VERSION}" '
    $0 ~ "^## \\[" version "\\]" { found = 1 }
    found && /^## \[/ && $0 !~ "^## \\[" version "\\]" { exit }
    found { print }
' "${CHANGELOG}"
