#!/usr/bin/env python3
"""Latency/throughput benchmark for `d.multicall2` (the Flood poll path).

Each call serializes a Flood-like column set for every visible download, so
this is where per-(download x command) parsing shows up.

Usage:
    python3 bench_multicall.py --sock /tmp/bench/session/bench.sock --iterations 20
"""

from __future__ import annotations

import argparse
import json
import time

import scgi_client

COLUMNS = [
    "d.hash=",
    "d.name=",
    "d.size_bytes=",
    "d.completed_bytes=",
    "d.bytes_done=",
    "d.up.rate=",
    "d.down.rate=",
    "d.ratio=",
    "d.is_active=",
    "d.state=",
    "d.is_hash_checking=",
    "d.peers_complete=",
]


def percentile(sorted_values, fraction: float) -> float:
    if not sorted_values:
        return 0.0
    index = min(len(sorted_values) - 1, int(round(fraction * (len(sorted_values) - 1))))
    return sorted_values[index]


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sock", required=True)
    parser.add_argument("--view", default="default")
    parser.add_argument("--iterations", type=int, default=20)
    parser.add_argument("--warmup", type=int, default=2)
    parser.add_argument("--json-out")
    args = parser.parse_args(argv)

    params = [args.view] + COLUMNS
    latencies = []
    rows = 0

    with scgi_client.Connection(args.sock, timeout=300) as connection:
        for _ in range(args.warmup):
            connection.call("d.multicall2", params)[1]

        for _ in range(args.iterations):
            started = time.perf_counter()
            body = connection.call("d.multicall2", params)[1]
            latencies.append(time.perf_counter() - started)
            rows = len(scgi_client.find_hashes(body))

    latencies.sort()
    result = {
        "view": args.view,
        "rows": rows,
        "columns": len(COLUMNS),
        "iterations": args.iterations,
        "p50_ms": percentile(latencies, 0.50) * 1000,
        "p95_ms": percentile(latencies, 0.95) * 1000,
        "p99_ms": percentile(latencies, 0.99) * 1000,
        "max_ms": (latencies[-1] if latencies else 0.0) * 1000,
        "rows_per_s": (rows / latencies[-1]) if latencies and latencies[-1] else 0.0,
    }
    print(json.dumps(result, indent=2))
    if args.json_out:
        with open(args.json_out, "w", encoding="utf-8") as handle:
            json.dump(result, handle, indent=2)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
