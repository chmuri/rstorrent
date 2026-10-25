# Baseline: startup, shutdown and session writes (2026-10-25)

Phase 0 measurements for the "slow start / hanging Ctrl+C" report. Every
number here comes from `RSTORRENT_METRICS_FILE` plus `/proc/<pid>/io` and a
before/after snapshot of the session directory; the raw JSON sits next to this
file in `2026-10-25-baseline-1k/` and `2026-10-25-baseline-10k/`.

Host: 8 vCPU, ext2/ext3 work filesystem, `nofile` 1048576, rstorrent 0.14.0,
release build. Payload torrents are 1 MiB sparse files so the run fits in the
test window; only startup/shutdown timings are being attributed here, not hash
throughput.

```bash
scripts/bench/run_all.sh --count 10000 --lookups 2000 --multicall 5 \
    --no-build --payload --shutdown --pty
```

## Verdicts on the four hypotheses

| Hypothesis | Verdict | Evidence |
|---|---|---|
| H1. The TUI waits on `initialize()` before drawing | **Falsified** | first frame 0.126 s at 1k, 0.544 s at 10k; `startup.total_ms` 576 ms at 10k |
| H2. Startup is slow because of 10k hash passes | **Partly** — restore itself is fast; rechecks are queued asynchronously and their cost lands in background I/O and per-event saves | `startup.total_ms` 596 ms with 10 000 rechecks queued |
| H3. A single Ctrl+C does not bound the process | **Confirmed** | `session_save` hits its 30 s timeout, then `lock_release` waits another 51–54 s |
| H4. Unchanged session files are rewritten | **Confirmed** | `files_rewritten` 30 001 and 122.9 MB written for an unchanged session |

## Startup

| Metric | 1k | 10k |
|---|---|---|
| `startup-pty.first_frame_s` | 0.126 s | 0.544 s |
| `startup.full_s` (all rows in `d.multicall2`) | 0.128 s | 0.728 s |
| `startup.total_ms` | 59 ms | 576 ms |
| `scan_entries` | 16 ms | 159 ms |
| `restore_entries` | 18 ms | 233 ms |
| `insert_downloads` | 17 ms | 176 ms |
| everything else | < 5 ms | < 5 ms |
| `insert_downloads.resume_rejected` | 0 | 0 |

Startup is linear and cheap: **~58 µs per torrent**. Nothing in the startup
path is worth optimising before shutdown is fixed.

Also falsified: the idea that the UI cannot come up. It comes up in half a
second and the terminal is usable immediately.

## The forced recheck

`startup-warm` is a restart where every torrent already has accepted resume
data (`resume_rejected: 0`) and its payload on disk:

| Metric | payload cold (no resume) | warm restart (valid resume) |
|---|---|---|
| `insert_downloads.resume_rejected` | 0 | 0 |
| `rechecks_queued` | 10 000 | **10 000** |

Cold is correct: no resume means the payload must be verified. Warm is the bug:
`insert_downloads` at `crates/rstorrent/src/control.rs` calls
`queue_recheck_if_files_exist()` unconditionally, so a trusted resume is thrown
away and all 10 000 torrents are rehashed again. This is what produces the
10 000 `hash_done` events, and each of those persists resume data immediately
(see below), which is why a "restart" turns into sustained disk writes.

## Shutdown

| Metric | 1k | 10k |
|---|---|---|
| `shutdown.exit_s` (SIGINT → process gone) | 6.63 s | **84.53 s** |
| `shutdown.lock_free_s` | 6.63 s | 84.53 s |
| `shutdown_phases_ms.network_drain` | 0 ms | 0 ms |
| `shutdown_phases_ms.flush_downloads` | 22 ms | 173 ms |
| `shutdown_phases_ms.session_save` | 6 542 ms | **30 001 ms (timeout)** |
| `shutdown_phases_ms.lock_release` | 0 ms | **54 291 ms** |
| `files_rewritten` | 3 001 | **30 001** |
| `bytes_on_disk_delta` | −15 B (lock file) | −15 B |
| `proc_write_bytes` during shutdown | 12.3 MB | **122.9 MB** |

Attribution at 10k: **36 % timeout + 61 % waiting for the session lock**, and
under 1 % for everything the shutdown path actually intends to do.

### Why `lock_release` takes a minute

`Control::save_session_state` takes `session_manager.lock()` once and holds it
while committing every entry. The 30 s `SESSION_SAVE_TIMEOUT` only stops
*waiting* — the `spawn_blocking` task keeps writing and keeps holding the
mutex. The next statement, `session.stop()`, then blocks on that same mutex for
another 40–54 s. The timeout therefore makes the shutdown longer, not shorter:
it adds 30 s of waiting on top of the same write that would have finished in
~65 s anyway.

### Write amplification

3 files per torrent (`<hash>.torrent`, `.libtorrent_resume`, `.rtorrent`) are
staged and renamed on every shutdown even when nothing changed, each with an
`fdatasync` and a directory `fsync`:

| Counter (10k) | Value |
|---|---|
| `session_entries` | 10 000 |
| `session_staged_files` | 30 000 |
| `session_fdatasync_calls` | 30 000 |
| `session_dir_sync_calls` | 10 000 |
| `session_bytes` (bencode payload) | 4.31 MB |
| `proc_write_bytes` (blocks on disk) | 122.9 MB |

The 29× gap between bencode bytes and bytes written is block padding plus the
per-file syncs: ~2.8 ms per file, ~8.4 ms per torrent.

### Data loss evidence

`shutdown-warm.files_added` is 4 378 at 10k: the previous shutdown hit its
timeout and the process was signalled out from under the writer, so the next
save had to create resume files that were never published. A bounded shutdown
must therefore **cancel** pending writes and persist a consistent prefix, not
simply stop waiting.

## Interactive mode

The PTY run reports both numbers that matter and they differ by two orders of
magnitude:

| Metric | 1k | 10k |
|---|---|---|
| `first_frame_s` | 0.126 s | 0.544 s |
| `exit_s` after one Ctrl+C | 6.68 s | 80.77 s |

The UI is fine. The exit path is the entire problem.

## What the fix has to move

| Target | Current (10k) | Measurement that proves it |
|---|---|---|
| Ctrl+C → exit | 84.5 s | `shutdown.exit_s` < 5 s (hard cap 10 s) |
| Session files written on an unchanged shutdown | 30 001 | `files_rewritten` = 0 |
| Bytes written on an unchanged shutdown | 122.9 MB | `proc_write_bytes` ≈ 0 |
| Rechecks on a trusted resume | 10 000 | `rechecks_queued` = 0 |
| TUI first frame | 0.544 s | already meets < 1 s |

The measurement channel stays in place, so each fix is accepted only when these
five numbers move. `run_all.sh --payload --shutdown --pty` reproduces this
baseline exactly.
