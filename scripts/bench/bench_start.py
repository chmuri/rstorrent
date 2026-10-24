#!/usr/bin/env python3
"""Measure rstorrent startup and steady-state footprint on a synthetic session.

Times three points:
  socket_s  - SCGI socket exists (listener bound)
  rpc_s     - system.client_version answers
  full_s    - d.multicall2 returns every expected row (restore finished)

While waiting it samples /proc for peak RSS, thread count and descriptor
count, which is the evidence behind the "one thread per recheck" problem.

Usage:
    python3 bench_start.py --session /tmp/bench/session --expected 14000
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import threading
import time

import scgi_client

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def read_status(pid: int) -> dict:
    values = {}
    try:
        with open(f"/proc/{pid}/status", encoding="utf-8") as handle:
            for line in handle:
                key, _, rest = line.partition(":")
                if key in ("VmRSS", "VmHWM", "Threads"):
                    values[key] = int(rest.strip().split()[0])
    except (FileNotFoundError, ProcessLookupError):
        pass
    return values


def read_fd_count(pid: int) -> int:
    try:
        return len(os.listdir(f"/proc/{pid}/fd"))
    except (FileNotFoundError, ProcessLookupError):
        return 0


def read_cpu_seconds(pid: int) -> float:
    try:
        with open(f"/proc/{pid}/stat", encoding="utf-8") as handle:
            fields = handle.read().split()
        ticks = int(fields[13]) + int(fields[14])
        return ticks / os.sysconf("SC_CLK_TCK")
    except (FileNotFoundError, ProcessLookupError, IndexError, ValueError):
        return 0.0


class Sampler(threading.Thread):
    def __init__(self, pid: int, interval: float = 0.02):
        super().__init__(daemon=True)
        self.pid = pid
        self.interval = interval
        self.stop_event = threading.Event()
        self.peak_rss = 0
        self.peak_threads = 0
        self.peak_fds = 0

    def run(self) -> None:
        while not self.stop_event.is_set():
            status = read_status(self.pid)
            self.peak_rss = max(self.peak_rss, status.get("VmRSS", 0))
            self.peak_threads = max(self.peak_threads, status.get("Threads", 0))
            self.peak_fds = max(self.peak_fds, read_fd_count(self.pid))
            self.stop_event.wait(self.interval)

    def stop(self) -> None:
        self.stop_event.set()
        self.join(timeout=2)


def row_count(sock: str) -> int:
    body = scgi_client.call(sock, "d.multicall2", ["default", "d.hash="], timeout=120)
    return len(scgi_client.find_hashes(body))


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--session", required=True)
    parser.add_argument("--data", help="download directory (default: sibling 'data')")
    parser.add_argument("--sock", help="SCGI socket path (default: <session>/bench.sock)")
    parser.add_argument("--expected", type=int, default=0, help="expected torrent count")
    parser.add_argument("--bin", default=os.path.join(REPO_ROOT, "target/release/rstorrent"))
    parser.add_argument("--port", type=int, default=50000)
    parser.add_argument("--disk-threads", type=int, default=0, help="set storage.disk_threads")
    parser.add_argument("--timeout", type=float, default=900.0)
    parser.add_argument(
        "--keep-running",
        action="store_true",
        help="leave the daemon running after measuring (for bench_lookup/recheck)",
    )
    parser.add_argument("--json-out")
    args = parser.parse_args(argv)

    session = os.path.abspath(args.session)
    data = os.path.abspath(args.data) if args.data else os.path.join(os.path.dirname(session), "data")
    sock = os.path.abspath(args.sock) if args.sock else os.path.join(session, "bench.sock")
    binary = os.path.abspath(args.bin)
    log_path = os.path.join(os.path.dirname(session), "rstorrent.log")

    for path in (sock,):
        try:
            os.unlink(path)
        except FileNotFoundError:
            pass
    os.makedirs(data, exist_ok=True)

    command = [
        binary,
        "-n",
        "-s",
        session,
        "-o",
        f"directory.default.set = {data}",
        "-o",
        f"network.scgi.open_local = {sock}",
        "-o",
        f"network.port_range.set = {args.port}-{args.port}",
    ]
    if args.disk_threads > 0:
        command.extend(["-o", f"storage.disk_threads = {args.disk_threads}"])
    command.append("--daemon")
    print("exec: " + " ".join(command), file=sys.stderr)

    with open(log_path, "wb") as log:
        process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT)
        sampler = Sampler(process.pid)
        sampler.start()
        started = time.monotonic()
        result = {"command": command, "pid": process.pid, "log": log_path}

        try:
            while not os.path.exists(sock):
                if process.poll() is not None:
                    raise RuntimeError(f"rstorrent exited early (see {log_path})")
                if time.monotonic() - started > args.timeout:
                    raise TimeoutError("SCGI socket never appeared")
                time.sleep(0.01)
            result["socket_s"] = time.monotonic() - started

            deadline = started + args.timeout
            while True:
                try:
                    scgi_client.call(sock, "system.client_version", timeout=5)
                    break
                except (OSError, RuntimeError):
                    if process.poll() is not None:
                        raise RuntimeError(f"rstorrent exited early (see {log_path})")
                    if time.monotonic() > deadline:
                        raise TimeoutError("XML-RPC never answered")
                    time.sleep(0.01)
            result["rpc_s"] = time.monotonic() - started

            if args.expected > 0:
                while True:
                    rows = row_count(sock)
                    if rows >= args.expected:
                        break
                    if process.poll() is not None:
                        raise RuntimeError(f"rstorrent exited during restore (see {log_path})")
                    if time.monotonic() > deadline:
                        raise TimeoutError(f"only {rows}/{args.expected} rows after timeout")
                    time.sleep(0.05)
                result["rows"] = rows
                result["full_s"] = time.monotonic() - started
            else:
                result["rows"] = row_count(sock)

            status = read_status(process.pid)
            result["final"] = {
                "rss_kb": status.get("VmRSS", 0),
                "threads": status.get("Threads", 0),
                "fds": read_fd_count(process.pid),
                "cpu_s": read_cpu_seconds(process.pid),
                "elapsed_s": time.monotonic() - started,
            }
        finally:
            if process.poll() is None:
                # A short startup can finish before the sampler's first tick.
                status = read_status(process.pid)
                sampler.peak_rss = max(sampler.peak_rss, status.get("VmRSS", 0))
                sampler.peak_threads = max(sampler.peak_threads, status.get("Threads", 0))
                sampler.peak_fds = max(sampler.peak_fds, read_fd_count(process.pid))
            sampler.stop()
            if process.poll() is None and not args.keep_running:
                process.send_signal(signal.SIGTERM)
                try:
                    process.wait(timeout=60)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)
            if args.keep_running and process.poll() is None:
                result["left_running"] = True
            result["peak"] = {
                "rss_kb": sampler.peak_rss,
                "threads": sampler.peak_threads,
                "fds": sampler.peak_fds,
            }

    print(json.dumps(result, indent=2))
    if args.json_out:
        with open(args.json_out, "w", encoding="utf-8") as handle:
            json.dump(result, handle, indent=2)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, TimeoutError) as error:
        print(f"bench_start: {error}", file=sys.stderr)
        raise SystemExit(1)
