#!/usr/bin/env python3
"""Generate a synthetic rtorrent session directory for startup benchmarks.

Each entry is a single-file torrent written as `<SESSION>/<HASH>.torrent`
with a hash that matches its file name, which is what the restore path
validates. No payload files are created, so restored downloads stay paused
and do not queue a hash check -- this isolates the restore/insert path that
Phases 1 and 2 change.

Usage:
    python3 gen_session.py --out /tmp/bench/session --count 14000
    python3 gen_session.py --out /tmp/bench/session --count 1000 --size-bytes 1048576
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time

from bencode import sha1, torrent_bytes


def piece_hashes(seed: int, index: int, count: int) -> bytes:
    pieces = b""
    for piece in range(count):
        pieces += sha1(f"{seed}:{index}:{piece}".encode())
    return pieces


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, help="session directory to create")
    parser.add_argument("--count", type=int, default=14000, help="number of torrents")
    parser.add_argument("--size-bytes", type=int, default=16384, help="size per torrent")
    parser.add_argument("--piece-size", type=int, default=16384, help="piece length")
    parser.add_argument("--seed", type=int, default=1)
    parser.add_argument("--manifest", help="manifest path (default: <out>/manifest.json)")
    args = parser.parse_args(argv)

    if args.count < 1:
        parser.error("--count must be positive")
    if args.size_bytes < 1 or args.piece_size < 1:
        parser.error("--size-bytes and --piece-size must be positive")

    os.makedirs(args.out, exist_ok=True)
    manifest_path = args.manifest or os.path.join(args.out, "manifest.json")

    num_pieces = (args.size_bytes + args.piece_size - 1) // args.piece_size
    entries = []
    started = time.monotonic()
    for index in range(args.count):
        name = f"bench-{index:06d}.bin"
        pieces = piece_hashes(args.seed, index, num_pieces)
        torrent, info_hash = torrent_bytes(name, args.size_bytes, args.piece_size, pieces)
        path = os.path.join(args.out, f"{info_hash}.torrent")
        with open(path, "wb") as handle:
            handle.write(torrent)
        entries.append(
            {
                "hash": info_hash,
                "name": name,
                "size": args.size_bytes,
                "piece_length": args.piece_size,
                "pieces": num_pieces,
            }
        )
        if (index + 1) % 1000 == 0 or index + 1 == args.count:
            print(f"\rgenerated {index + 1}/{args.count}", end="", file=sys.stderr, flush=True)

    with open(manifest_path, "w", encoding="utf-8") as handle:
        json.dump({"seed": args.seed, "entries": entries}, handle)
    elapsed = time.monotonic() - started
    print(
        f"\n{args.count} torrents in {args.out} ({elapsed:.2f}s), manifest {manifest_path}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
