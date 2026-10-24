#!/usr/bin/env python3
"""Latency benchmark for `d.hash` lookups (the DownloadList index hot path).

Reports p50/p95/p99/max over a keep-alive SCGI connection. Hits come from the
session manifest; misses are random 40-hex hashes, which is the worst case for
a linear scan and the reason Phase 1 exists.

Usage:
    python3 bench_lookup.py --sock /tmp/bench/session/bench.sock \
        --manifest /tmp/bench/session/manifest.json --iterations 10000
"""

from __future__ import annotations

import argparse
import json
import random
import statistics
import threading
import time

import scgi_client


def percentile(sorted_values, fraction: float) -> float:
    if not sorted_values:
        return 0.0
    index = min(len(sorted_values) - 1, int(round(fraction * (len(sorted_values) - 1))))
    return sorted_values[index]


def run_worker(sock: str, hashes, iterations: int, miss_ratio: float, seed: int, sink: list):
    rng = random.Random(seed)
    latencies = []
    errors = 0
    faults = 0
    hits = 0

    connection = scgi_client.Connection(sock)
    for _ in range(iterations):
        if rng.random() < miss_ratio:
            target = f"{rng.getrandbits(160):040X}"
        else:
            target = rng.choice(hashes)
            hits += 1

        started = time.perf_counter()
        try:
            body = connection.call("d.hash", [target])[1]
        except (OSError, ConnectionError):
            errors += 1
            try:
                connection.close()
            except OSError:
                pass
            connection = scgi_client.Connection(sock)
            continue
        elapsed = time.perf_counter() - started
        latencies.append(elapsed)
        if scgi_client.is_fault(body):
            # A miss is an XML-RPC fault; its latency is still meaningful.
            faults += 1
    connection.close()
    sink.append((latencies, hits, faults, errors))


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sock", required=True)
    parser.add_argument("--manifest", required=True)
    parser.add_argument("--iterations", type=int, default=10000)
    parser.add_argument("--miss-ratio", type=float, default=0.1)
    parser.add_argument("--concurrency", type=int, default=1)
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--json-out")
    args = parser.parse_args(argv)

    with open(args.manifest, encoding="utf-8") as handle:
        hashes = [entry["hash"] for entry in json.load(handle)["entries"]]
    if not hashes:
        parser.error("manifest has no entries")

    per_worker = max(1, args.iterations // args.concurrency)
    sink = []
    threads = [
        threading.Thread(
            target=run_worker,
            args=(args.sock, hashes, per_worker, args.miss_ratio, args.seed + index, sink),
        )
        for index in range(args.concurrency)
    ]

    started = time.perf_counter()
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    elapsed = time.perf_counter() - started

    latencies = sorted(value for worker in sink for value in worker[0])
    errors = sum(worker[3] for worker in sink)
    faults = sum(worker[2] for worker in sink)
    hits = sum(worker[1] for worker in sink)
    result = {
        "calls": len(latencies),
        "hits": hits,
        "faults": faults,
        "errors": errors,
        "concurrency": args.concurrency,
        "miss_ratio": args.miss_ratio,
        "wall_s": elapsed,
        "qps": len(latencies) / elapsed if elapsed else 0.0,
        "p50_ms": percentile(latencies, 0.50) * 1000,
        "p95_ms": percentile(latencies, 0.95) * 1000,
        "p99_ms": percentile(latencies, 0.99) * 1000,
        "max_ms": (latencies[-1] if latencies else 0.0) * 1000,
        "mean_ms": (statistics.fmean(latencies) * 1000) if latencies else 0.0,
    }
    print(json.dumps(result, indent=2))
    if args.json_out:
        with open(args.json_out, "w", encoding="utf-8") as handle:
            json.dump(result, handle, indent=2)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
