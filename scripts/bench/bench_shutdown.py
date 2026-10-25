#!/usr/bin/env python3
"""Measure graceful shutdown: latency, lock release and session rewrites.

Starts the daemon on an existing session, waits until every torrent is visible
over SCGI, then sends SIGINT and records:

  exit_s             signal -> process exit
  lock_free_s        signal -> `<session>/rtorrent.lock` is gone
  files_rewritten    session files whose inode or mtime changed
  files_added        new session files (resume data for first-time saves)
  bytes_on_disk      size delta of the session directory
  proc_write_bytes   bytes written between "steady state" and exit
  phase_ms           shutdown phases from RSTORRENT_METRICS_FILE

A non-zero `files_rewritten` after an unchanged session is exactly the write
amplification this benchmark exists to expose.

Usage:
    python3 bench_shutdown.py --session /tmp/bench/session --expected 14000
    python3 bench_shutdown.py --session ... --json-out /tmp/shutdown.json
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import stat
import subprocess
import sys
import time

import scgi_client
from bench_start import read_metrics, row_count

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def snapshot_dir(path: str) -> dict[str, tuple[int, int, int]]:
    """Maps every regular file name to (mtime_ns, inode, size)."""
    snapshot: dict[str, tuple[int, int, int]] = {}
    try:
        names = os.listdir(path)
    except FileNotFoundError:
        return snapshot
    for name in names:
        try:
            st = os.stat(os.path.join(path, name))
        except FileNotFoundError:
            continue
        if not stat.S_ISREG(st.st_mode):
            continue
        snapshot[name] = (st.st_mtime_ns, st.st_ino, st.st_size)
    return snapshot


def diff_snapshots(
    before: dict[str, tuple[int, int, int]], after: dict[str, tuple[int, int, int]]
) -> dict[str, int]:
    rewritten = sum(
        1
        for name, meta in before.items()
        if name in after and after[name][:2] != meta[:2]
    )
    added = sum(1 for name in after if name not in before)
    removed = sum(1 for name in before if name not in after)
    bytes_before = sum(meta[2] for meta in before.values())
    bytes_after = sum(meta[2] for meta in after.values())
    return {
        "files_before": len(before),
        "files_after": len(after),
        "files_rewritten": rewritten,
        "files_added": added,
        "files_removed": removed,
        "bytes_on_disk_delta": bytes_after - bytes_before,
    }


def read_write_bytes(pid: int) -> int | None:
    try:
        with open(f"/proc/{pid}/io", encoding="utf-8") as handle:
            for line in handle:
                if line.startswith("write_bytes:"):
                    return int(line.split(":", 1)[1])
    except (FileNotFoundError, ProcessLookupError, ValueError):
        return None
    return None


def lock_removed(session: str) -> bool:
    lock_path = os.path.join(session, "rtorrent.lock")
    if not os.path.exists(lock_path):
        return True
    # The lock is a pid file: a removed file means released, a stale pid means
    # the previous owner is gone.
    try:
        with open(lock_path, encoding="utf-8", errors="replace") as handle:
            content = handle.read().strip()
        pid = int(content.rsplit("+", 1)[1])
    except (OSError, IndexError, ValueError):
        return False
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return True
    except PermissionError:
        return False
    return False


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--session", required=True)
    parser.add_argument("--data", help="download directory (default: sibling 'data')")
    parser.add_argument("--sock", help="SCGI socket (default: <session>/bench.sock)")
    parser.add_argument("--expected", type=int, default=0)
    parser.add_argument("--bin", default=os.path.join(REPO_ROOT, "target/release/rstorrent"))
    parser.add_argument("--port", type=int, default=50100)
    parser.add_argument("--signal", default="INT", choices=("INT", "TERM"))
    parser.add_argument("--settle", type=float, default=1.0, help="idle seconds before signalling")
    parser.add_argument("--timeout", type=float, default=120.0)
    parser.add_argument("--metrics", help="RSTORRENT_METRICS_FILE path")
    parser.add_argument("--json-out")
    args = parser.parse_args(argv)

    session = os.path.abspath(args.session)
    data = os.path.abspath(args.data) if args.data else os.path.join(os.path.dirname(session), "data")
    sock = os.path.abspath(args.sock) if args.sock else os.path.join(session, "bench.sock")
    binary = os.path.abspath(args.bin)
    metrics_path = os.path.abspath(args.metrics) if args.metrics else None
    log_path = os.path.join(os.path.dirname(session), "rstorrent-shutdown.log")

    os.makedirs(data, exist_ok=True)
    try:
        os.unlink(sock)
    except FileNotFoundError:
        pass

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
        "--daemon",
    ]

    env = dict(os.environ)
    if metrics_path:
        env["RSTORRENT_METRICS_FILE"] = metrics_path

    result: dict = {"command": command, "pid": None}
    with open(log_path, "wb") as log:
        process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, env=env)
        result["pid"] = process.pid
        started = time.monotonic()
        try:
            while not os.path.exists(sock):
                if process.poll() is not None:
                    raise RuntimeError(f"rstorrent exited during startup (see {log_path})")
                if time.monotonic() - started > args.timeout:
                    raise TimeoutError("SCGI socket never appeared")
                time.sleep(0.01)
            result["socket_s"] = time.monotonic() - started

            deadline = started + args.timeout
            while True:
                try:
                    rows = row_count(sock)
                except (OSError, RuntimeError):
                    rows = 0
                if args.expected <= 0 or rows >= args.expected:
                    break
                if process.poll() is not None:
                    raise RuntimeError("rstorrent exited during restore")
                if time.monotonic() > deadline:
                    raise TimeoutError(f"only {rows}/{args.expected} rows after timeout")
                time.sleep(0.05)
            result["rows"] = rows
            result["full_s"] = time.monotonic() - started

            time.sleep(args.settle)
            before = snapshot_dir(session)
            write_before = read_write_bytes(process.pid)

            signal_at = time.monotonic()
            process.send_signal(
                signal.SIGINT if args.signal == "INT" else signal.SIGTERM
            )

            try:
                process.wait(timeout=args.timeout)
                result["timed_out"] = False
            except subprocess.TimeoutExpired:
                result["timed_out"] = True
                process.kill()
                process.wait(timeout=10)
            result["exit_s"] = time.monotonic() - signal_at
            result["returncode"] = process.returncode

            lock_deadline = time.monotonic() + 5.0
            while not lock_removed(session) and time.monotonic() < lock_deadline:
                time.sleep(0.01)
            result["lock_free_s"] = time.monotonic() - signal_at
            result["lock_released"] = lock_removed(session)

            after = snapshot_dir(session)
            result.update(diff_snapshots(before, after))

            metrics = read_metrics(metrics_path) if metrics_path else None
            result["metrics"] = metrics
            summary = (metrics or {}).get("summaries", {}).get("shutdown.summary", {})
            if write_before is not None and "proc_write_bytes" in summary:
                result["proc_write_bytes"] = summary["proc_write_bytes"] - write_before
            result["shutdown_phases_ms"] = (metrics or {}).get("shutdown_phases_ms", {})
            save_event = (metrics or {}).get("shutdown_phase_events", {}).get("session_save", {})
            result["session_save_timed_out"] = bool(save_event.get("timed_out", 0))
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=10)

    print(json.dumps(result, indent=2))
    if args.json_out:
        with open(args.json_out, "w", encoding="utf-8") as handle:
            json.dump(result, handle, indent=2)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, TimeoutError) as error:
        print(f"bench_shutdown: {error}", file=sys.stderr)
        raise SystemExit(1)
