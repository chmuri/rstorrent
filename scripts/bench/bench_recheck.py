#!/usr/bin/env python3
"""Recheck concurrency/throughput benchmark.

Triggers `d.check_hash` for K torrents (each call currently spawns one OS
thread in DownloadList::begin_hash_check) and polls until every pass drains.

Reports wall time, MB/s, peak concurrent checks and peak thread count -- the
evidence for Phase 5's bounded scheduler.

Usage:
    python3 bench_recheck.py --sock /tmp/bench/session/bench.sock \
        --pid 12345 --manifest /tmp/bench/session/payload-manifest.json --count 100
"""

from __future__ import annotations

import argparse
import json
import time

import scgi_client


def thread_count(pid: int) -> int:
    try:
        with open(f"/proc/{pid}/status", encoding="utf-8") as handle:
            for line in handle:
                if line.startswith("Threads:"):
                    return int(line.split()[1])
    except (FileNotFoundError, ProcessLookupError):
        pass
    return 0


def active_checks(sock: str) -> int:
    body = scgi_client.call(sock, "d.multicall2", ["default", "d.is_hash_checking="], timeout=120)
    return sum(1 for value in scgi_client.parse_values(body) if value.strip() == "1")


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sock", required=True)
    parser.add_argument("--pid", type=int, required=True, help="rstorrent pid (for thread sampling)")
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--count", type=int, default=0, help="how many torrents (0 = all)")
    parser.add_argument("--poll-interval", type=float, default=0.2)
    parser.add_argument(
        "--grace",
        type=float,
        default=2.0,
        help="if no pass is observed this soon after triggering, assume they completed",
    )
    parser.add_argument("--timeout", type=float, default=3600.0)
    parser.add_argument("--json-out")
    args = parser.parse_args(argv)

    with open(args.manifest, encoding="utf-8") as handle:
        entries = json.load(handle)["entries"]
    if args.count > 0:
        entries = entries[: args.count]
    if not entries:
        parser.error("no entries to recheck")

    total_bytes = sum(entry["size"] for entry in entries)
    base_threads = thread_count(args.pid)

    started = time.perf_counter()
    with scgi_client.Connection(args.sock, timeout=300) as connection:
        for entry in entries:
            connection.call("d.check_hash", [entry["hash"]])
    triggered = time.perf_counter()

    peak_active = 0
    peak_threads = base_threads
    seen_active = False
    deadline = started + args.timeout

    while True:
        try:
            active = active_checks(args.sock)
        except (OSError, ConnectionError):
            active = 0
        peak_active = max(peak_active, active)
        peak_threads = max(peak_threads, thread_count(args.pid))
        if active > 0:
            seen_active = True
        if seen_active and active == 0:
            break
        if not seen_active and time.perf_counter() - triggered > args.grace:
            # A tiny payload can finish between the trigger and the first
            # sample; do not hang waiting for a state that already passed.
            break
        if time.perf_counter() > deadline:
            raise TimeoutError(f"{active} rechecks still running after {args.timeout}s")
        time.sleep(args.poll_interval)

    finished = time.perf_counter()
    wall = finished - started
    result = {
        "torrents": len(entries),
        "bytes": total_bytes,
        "trigger_s": triggered - started,
        "wall_s": wall,
        "mb_per_s": total_bytes / 1e6 / wall if wall else 0.0,
        "peak_active_checks": peak_active,
        "observed_active": seen_active,
        "base_threads": base_threads,
        "peak_threads": peak_threads,
        "final_threads": thread_count(args.pid),
    }
    print(json.dumps(result, indent=2))
    if args.json_out:
        with open(args.json_out, "w", encoding="utf-8") as handle:
            json.dump(result, handle, indent=2)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except TimeoutError as error:
        print(f"bench_recheck: {error}")
        raise SystemExit(1)
