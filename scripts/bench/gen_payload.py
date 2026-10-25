#!/usr/bin/env python3
"""Generate torrents with real payload files for recheck/throughput benchmarks.

Payload files are sparse by default: a 64 MiB torrent costs almost no disk
and still exercises the read path (holes read as zeros). Pass `--fill` to
write a deterministic byte pattern instead.

Torrents are written without a `.libtorrent_resume`, so the restore path
queues a hash check for the existing payload. That is intentional: it models
"1000 torrents need rechecking after a restart", which is the Phase 5 load.

Usage:
    python3 gen_payload.py --out /tmp/bench/session --data /tmp/bench/data \
        --count 20 --size-mb 64
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import time

import bencode
from bencode import torrent_bytes

PATTERN = bytes(range(256))


def piece_bytes(length: int, offset: int, fill: bool) -> bytes:
    if not fill:
        return b"\x00" * length
    start = offset & 0xFF
    cycle = PATTERN[start:] + PATTERN[:start]
    return (cycle * (length // len(cycle) + 1))[:length]


def piece_digests(size: int, piece_size: int, fill: bool) -> bytes:
    digests = b""
    offset = 0
    while offset < size:
        length = min(piece_size, size - offset)
        digests += hashlib.sha1(piece_bytes(length, offset, fill)).digest()
        offset += length
    return digests


def write_payload(path: str, size: int, fill: bool, piece_size: int) -> None:
    if not fill:
        # Sparse file: instant, reads back as zeros.
        with open(path, "wb") as handle:
            handle.truncate(size)
        return

    with open(path, "wb") as handle:
        offset = 0
        while offset < size:
            length = min(piece_size, size - offset)
            handle.write(piece_bytes(length, offset, fill))
            offset += length


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, help="session directory")
    parser.add_argument("--data", required=True, help="payload directory (save path)")
    parser.add_argument("--count", type=int, default=20)
    parser.add_argument("--size-mb", type=int, default=64)
    parser.add_argument("--piece-size", type=int, default=262144)
    parser.add_argument("--seed", type=int, default=2)
    parser.add_argument("--fill", action="store_true", help="write real bytes instead of sparse")
    parser.add_argument(
        "--active",
        action="store_true",
        help="write a .rtorrent sidecar marking the torrent active, so restore "
        "starts it and the queued recheck actually runs",
    )
    parser.add_argument("--manifest", help="manifest path (default: <out>/payload-manifest.json)")
    args = parser.parse_args(argv)

    size = args.size_mb * 1024 * 1024
    if args.count < 1 or size < 1 or args.piece_size < 1:
        parser.error("--count, --size-mb and --piece-size must be positive")

    os.makedirs(args.out, exist_ok=True)
    os.makedirs(args.data, exist_ok=True)
    manifest_path = args.manifest or os.path.join(args.out, "payload-manifest.json")

    entries = []
    started = time.monotonic()
    for index in range(args.count):
        name = f"payload-{index:06d}.bin"
        digests = piece_digests(size, args.piece_size, args.fill)
        torrent, info_hash = torrent_bytes(name, size, args.piece_size, digests)

        write_payload(os.path.join(args.data, name), size, args.fill, args.piece_size)
        with open(os.path.join(args.out, f"{info_hash}.torrent"), "wb") as handle:
            handle.write(torrent)
        if args.active:
            sidecar = {"directory": args.data, "state": 1}
            with open(os.path.join(args.out, f"{info_hash}.torrent.rtorrent"), "wb") as handle:
                handle.write(bencode.encode(sidecar))

        entries.append(
            {
                "hash": info_hash,
                "name": name,
                "size": size,
                "piece_length": args.piece_size,
                "pieces": len(digests) // 20,
            }
        )
        if (index + 1) % 10 == 0 or index + 1 == args.count:
            print(f"\rgenerated {index + 1}/{args.count}", end="", file=sys.stderr, flush=True)

    with open(manifest_path, "w", encoding="utf-8") as handle:
        json.dump({"seed": args.seed, "entries": entries}, handle)
    elapsed = time.monotonic() - started
    total = size * args.count
    print(
        f"\n{args.count} torrents, {total / 1e9:.2f} GB in {elapsed:.1f}s "
        f"({total / 1e6 / max(elapsed, 1e-9):.1f} MB/s)",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
