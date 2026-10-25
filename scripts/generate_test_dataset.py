#!/usr/bin/env python3
"""
Test Dataset Generator for rstorrent Benchmark

Generates N random binary files and matching valid bencoded single-file .torrent files
with trackers extracted from the specified magnet link:
  udp://tracker.opentrackr.org:1337/announce
  udp://open.stealth.si:80/announce
  udp://tracker.torrent.eu.org:451/announce
  udp://exodus.desync.com:6969/announce
"""

import argparse
import hashlib
import os
import sys
import time
from concurrent.futures import ProcessPoolExecutor, as_completed
from pathlib import Path

TRACKERS = [
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://exodus.desync.com:6969/announce",
]

def bencode(val):
    """Encodes a Python object into canonical BitTorrent bencode bytes."""
    if isinstance(val, int):
        return f"i{val}e".encode("ascii")
    elif isinstance(val, (bytes, bytearray)):
        return f"{len(val)}:".encode("ascii") + bytes(val)
    elif isinstance(val, str):
        b = val.encode("utf-8")
        return f"{len(b)}:".encode("ascii") + b
    elif isinstance(val, list):
        return b"l" + b"".join(bencode(x) for x in val) + b"e"
    elif isinstance(val, dict):
        items = []
        for k in sorted(val.keys(), key=lambda x: x if isinstance(x, bytes) else x.encode("utf-8")):
            k_bytes = k if isinstance(k, bytes) else k.encode("utf-8")
            items.append(f"{len(k_bytes)}:".encode("ascii") + k_bytes)
            items.append(bencode(val[k]))
        return b"d" + b"".join(items) + b"e"
    else:
        raise TypeError(f"Cannot bencode {type(val)}")


def generate_batch(start_idx, end_idx, files_dir, torrents_dir, file_size, piece_length, creation_time):
    """Generates a batch of files and their corresponding .torrent files."""
    count = 0
    announce_list = [[tr] for tr in TRACKERS]
    primary_announce = TRACKERS[0]

    for i in range(start_idx, end_idx):
        file_name = f"test_{i:05d}.bin"
        torrent_name = f"test_{i:05d}.torrent"

        payload_path = os.path.join(files_dir, file_name)
        torrent_path = os.path.join(torrents_dir, torrent_name)

        # Generate unique random content
        content = os.urandom(file_size)

        # Write payload file
        with open(payload_path, "wb") as f:
            f.write(content)

        # Compute SHA-1 for each piece
        pieces = []
        for offset in range(0, file_size, piece_length):
            chunk = content[offset:offset + piece_length]
            pieces.append(hashlib.sha1(chunk).digest())
        pieces_bytes = b"".join(pieces)

        # Build torrent dictionary
        torrent_dict = {
            "announce": primary_announce,
            "announce-list": announce_list,
            "created by": "rstorrent-test-generator",
            "creation date": creation_time,
            "info": {
                "length": file_size,
                "name": file_name,
                "piece length": piece_length,
                "pieces": pieces_bytes,
            },
        }

        with open(torrent_path, "wb") as f:
            f.write(bencode(torrent_dict))

        count += 1

    return count


def main():
    parser = argparse.ArgumentParser(description="Generate random files and matching .torrent files for benchmark.")
    parser.add_argument("--count", type=int, default=10000, help="Number of files/torrents to generate (default: 10000)")
    parser.add_argument("--size-kb", type=int, default=32, help="Payload file size in KB (default: 32)")
    parser.add_argument("--piece-size-kb", type=int, default=16, help="Piece length in KB (default: 16)")
    parser.add_argument("--files-dir", type=str, default="files", help="Destination directory for payload files (default: files)")
    parser.add_argument("--torrents-dir", type=str, default="test", help="Destination directory for .torrent files (default: test)")
    parser.add_argument("--workers", type=int, default=min(8, os.cpu_count() or 4), help="Number of worker processes")

    args = parser.parse_args()

    file_size = args.size_kb * 1024
    piece_length = args.piece_size_kb * 1024

    os.makedirs(args.files_dir, exist_ok=True)
    os.makedirs(args.torrents_dir, exist_ok=True)

    print(f"Generating {args.count} files ({args.size_kb} KB each, total {(args.count * args.size_kb) / 1024:.2f} MB)...")
    print(f"Payload destination:  {os.path.abspath(args.files_dir)}")
    print(f"Torrent destination:  {os.path.abspath(args.torrents_dir)}")
    print(f"Trackers:             {', '.join(TRACKERS)}")

    start_time = time.time()
    creation_time = int(start_time)

    batch_size = max(100, args.count // (args.workers * 4))
    ranges = []
    for s in range(0, args.count, batch_size):
        e = min(s + batch_size, args.count)
        ranges.append((s, e))

    total_done = 0
    with ProcessPoolExecutor(max_workers=args.workers) as executor:
        futures = [
            executor.submit(
                generate_batch,
                s,
                e,
                args.files_dir,
                args.torrents_dir,
                file_size,
                piece_length,
                creation_time,
            )
            for (s, e) in ranges
        ]

        for future in as_completed(futures):
            res = future.result()
            total_done += res
            if total_done % 1000 == 0 or total_done == args.count:
                elapsed = time.time() - start_time
                rate = total_done / elapsed if elapsed > 0 else 0
                print(f"Progress: {total_done}/{args.count} ({rate:.1f} items/sec)")

    elapsed = time.time() - start_time
    print(f"Successfully generated {total_done} payload files and {total_done} .torrent files in {elapsed:.2f} seconds.")


if __name__ == "__main__":
    main()
