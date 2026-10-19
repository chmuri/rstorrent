#!/usr/bin/env bash
#
# Builds release packages for a single target:
#   - rstorrent-<version>-<target>.tar.gz  (binary + docs + example config)
#   - rstorrent_<version>_<arch>.deb       (Linux targets only)
#   - SHA256SUMS
#
# Usage: scripts/package.sh <version> <target-triple> <binary> [output-dir]
set -euo pipefail

VERSION="${1:?usage: package.sh <version> <target> <binary> [output-dir]}"
TARGET="${2:?}"
BINARY="${3:?}"
OUT_DIR="${4:-dist}"

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PACKAGE_NAME="rstorrent"
TARBALL="${PACKAGE_NAME}-${VERSION}-${TARGET}.tar.gz"

mkdir -p "${OUT_DIR}"

STAGE_DIR="$(mktemp -d)"
trap 'rm -rf "${STAGE_DIR}"' EXIT

# --- Portable tarball -------------------------------------------------------
mkdir -p "${STAGE_DIR}/${PACKAGE_NAME}-${VERSION}"
install -m 0755 "${BINARY}" "${STAGE_DIR}/${PACKAGE_NAME}-${VERSION}/rstorrent"
ln -s rstorrent "${STAGE_DIR}/${PACKAGE_NAME}-${VERSION}/rtorrent"

for doc in README.md CHANGELOG.md LICENSE CONTRIBUTING.md; do
    if [ -f "${ROOT_DIR}/${doc}" ]; then
        install -m 0644 "${ROOT_DIR}/${doc}" "${STAGE_DIR}/${PACKAGE_NAME}-${VERSION}/${doc}"
    fi
done

if [ -f "${ROOT_DIR}/rstorrent.default.rc" ]; then
    install -m 0644 "${ROOT_DIR}/rstorrent.default.rc" "${STAGE_DIR}/${PACKAGE_NAME}-${VERSION}/rstorrent.default.rc"
fi

tar -C "${STAGE_DIR}" -czf "${OUT_DIR}/${TARBALL}" "${PACKAGE_NAME}-${VERSION}"
echo "created ${OUT_DIR}/${TARBALL}"

# --- Debian package (Linux only) -------------------------------------------
case "${TARGET}" in
    x86_64-unknown-linux-gnu | x86_64-unknown-linux-musl) DEB_ARCH="amd64" ;;
    aarch64-unknown-linux-gnu | aarch64-unknown-linux-musl) DEB_ARCH="arm64" ;;
    *) DEB_ARCH="" ;;
esac

if [ -n "${DEB_ARCH}" ]; then
    DEB_ROOT="${STAGE_DIR}/deb"
    mkdir -p "${DEB_ROOT}/DEBIAN" \
        "${DEB_ROOT}/usr/bin" \
        "${DEB_ROOT}/usr/share/doc/${PACKAGE_NAME}" \
        "${DEB_ROOT}/etc/rstorrent"

    install -m 0755 "${BINARY}" "${DEB_ROOT}/usr/bin/rstorrent"
    ln -s rstorrent "${DEB_ROOT}/usr/bin/rtorrent"

    install -m 0644 "${ROOT_DIR}/README.md" "${DEB_ROOT}/usr/share/doc/${PACKAGE_NAME}/README.md"
    install -m 0644 "${ROOT_DIR}/CHANGELOG.md" "${DEB_ROOT}/usr/share/doc/${PACKAGE_NAME}/CHANGELOG.md"
    install -m 0644 "${ROOT_DIR}/LICENSE" "${DEB_ROOT}/usr/share/doc/${PACKAGE_NAME}/LICENSE"
    install -m 0644 "${ROOT_DIR}/rstorrent.default.rc" "${DEB_ROOT}/usr/share/doc/${PACKAGE_NAME}/rstorrent.default.rc"
    install -m 0644 "${ROOT_DIR}/rstorrent.default.rc" "${DEB_ROOT}/etc/rstorrent/rstorrent.default.rc"

    INSTALLED_SIZE="$(du -sk --exclude=DEBIAN "${DEB_ROOT}" 2>/dev/null | awk '{print $1}')"
    if [ -z "${INSTALLED_SIZE}" ]; then
        INSTALLED_SIZE=1
    fi

    cat > "${DEB_ROOT}/DEBIAN/control" <<EOF
Package: ${PACKAGE_NAME}
Version: ${VERSION}
Section: net
Priority: optional
Architecture: ${DEB_ARCH}
Installed-Size: ${INSTALLED_SIZE}
Maintainer: chmuri <chmuri@gmail.com>
Homepage: https://github.com/chmuri/rstorrent
Depends: libc6 (>= 2.31), ca-certificates
Recommends: flood
Description: High-performance BitTorrent client and engine written in Rust
 rstorrent is a memory-safe, asynchronous re-implementation of rTorrent and
 libtorrent. It speaks SCGI, XML-RPC, JSON-RPC and a native gRPC API, ships an
 interactive TUI and a headless daemon, and integrates with Flood and ruTorrent.
 .
 The package provides both the rstorrent and rtorrent binaries.
EOF

    dpkg-deb --build --root-owner-group "${DEB_ROOT}" \
        "${OUT_DIR}/${PACKAGE_NAME}_${VERSION}_${DEB_ARCH}.deb" >/dev/null
    echo "created ${OUT_DIR}/${PACKAGE_NAME}_${VERSION}_${DEB_ARCH}.deb"
fi

# --- Checksums --------------------------------------------------------------
checksum() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$@"
    else
        # macOS/BSD
        shasum -a 256 "$@"
    fi
}

(
    cd "${OUT_DIR}"
    checksum "${TARBALL}" >"${PACKAGE_NAME}-${VERSION}-${TARGET}.sha256"
    if [ -n "${DEB_ARCH}" ]; then
        checksum "${PACKAGE_NAME}_${VERSION}_${DEB_ARCH}.deb" >>"${PACKAGE_NAME}-${VERSION}-${TARGET}.sha256"
    fi
)
echo "created ${OUT_DIR}/${PACKAGE_NAME}-${VERSION}-${TARGET}.sha256"
