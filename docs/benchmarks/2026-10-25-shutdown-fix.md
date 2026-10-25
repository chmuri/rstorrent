# Shutdown fix: bounded, lock-free, no forced recheck (2026-10-25)

Follow-up to [`2026-10-25-startup-shutdown-baseline.md`](2026-10-25-startup-shutdown-baseline.md).
Raw results: `2026-10-25-after-shutdown-fix-1k/`, `2026-10-25-after-shutdown-fix-10k/`.

## What changed

1. **The session mutex is no longer held across the write.** `save_session_state`
   snapshots a `SessionWriter` (path + `use_fsyncdisk`) under the lock, then
   releases it and writes. `SessionManager::stop()` therefore returns
   immediately instead of waiting for the whole commit.
2. **Entries are written across a small worker pool** (`min(cores, 8)`), each
   entry keeping its own `fdatasync` and directory `fsync` — the per-file
   durability semantics are unchanged, only the number of entries in flight.
3. **A timed-out save is actually stopped.** The writer checks an abort flag
   between entries and the shutdown waits up to 2 s for it instead of detaching
   a task that keeps writing behind the released lock.
4. **Restore no longer overrides a trusted resume.** The blanket
   `queue_recheck_if_files_exist()` is now gated: entries with no resume record,
   or with one that completed no pieces, still verify the payload.
5. **A verification in flight no longer persists an empty bitfield.** Such a
   download writes only its static `.torrent` (when missing); its resume is left
   to the next completed pass. This is the same rule the periodic checkpoint
   already used.
6. **The periodic checkpoint uses the same writer**, so a background checkpoint
   can no longer block `stop()`.

## Intermediate: parallel writer only

This was the state before dirty-only writes landed; it isolates the effect of
taking the session mutex out of the write path.

| Metric (10 000 torrents) | Baseline | After | |
|---|---|---|---|
| `shutdown.exit_s` | 84.53 s | **15.05 – 16.96 s** | 5.2× |
| `shutdown_phases_ms.session_save` | 30 001 (timeout) | 15 565 – 16 694 | no timeout |
| `shutdown_phases_ms.lock_release` | 54 291 | **0** | lock no longer contended |
| `session_save_timed_out` | true | **false** | |
| `files_rewritten` | 30 001 | 30 001 | unchanged (no dirty-only) |
| `proc_write_bytes` | 122.9 MB | 122.9 MB | unchanged |
| `startup-pty.first_frame_s` | 0.544 s | 0.67 – 0.74 s | unchanged, still < 1 s |
| `startup-pty.exit_s` | 80.77 s | **15.05 – 17.10 s** | 4.8× |

| Metric (1 000 torrents) | Baseline | After | |
|---|---|---|---|
| `shutdown.exit_s` | 6.63 s | **1.32 – 1.62 s** | 4.1× |
| `session_save` | 6 542 ms | 1 229 – 1 305 ms | 5.1× |
| `lock_release` | 0 ms | 0 ms | |
| `files_rewritten` | 3 001 | 3 001 | |

Throughput after the change: 30 001 files in ~16 s ≈ 1 800 files/s across 8
workers (~0.57 ms per file), versus ~360 files/s sequential.

Shutdown is now proportional to `entries / workers` and, critically, **does not
grow with the timeout**: the `lock_release` phase is zero because the lock is
free while the writer works.

## The recheck correction

The baseline report stated that `rechecks_queued = 10 000` on a warm restart
proved the forced recheck. That measurement was confounded, and the correction
matters:

- `resume_rejected = 0` only means the resume file parsed. In that fixture most
  torrents still had **zero completed pieces** (their verification had not run
  yet), which legitimately triggers a recheck through `restore_entry_at` —
  independent of the blanket call in `Control`.
- The blanket call in `Control` was real and did force a recheck even for a
  trustworthy resume, but the 10 000 figure could not isolate it.

The fixed end-to-end numbers, with sidecars that mark the torrents active so the
verification actually runs (`gen_payload.py --active`):

| 1k restart | rechecks |
|---|---|
| baseline | 1 000 |
| after | **~200** |

The remaining ~200 are torrents whose verification was still running at
shutdown; they are now skipped rather than persisted with an empty bitfield, so
they verify again on the next start. That is the intended trade-off, not the
blanket override.

The deterministic proof lives in
`crates/rstorrent/tests/test_restore_recheck.rs`:

- a resume with a validated piece ⇒ `queued_rechecks == 0`
- the same session without the resume file ⇒ `queued_rechecks == 1`

and in `crates/rt-core/tests/test_resume_no_forced_recheck.rs`, which pins the
restore-level facts the gate depends on (`resume_loaded`, `completed_chunks`).

## Dirty-only writes

The parallel writer fixed the *locking*, but an unchanged session still cost
~16 s at 10k because it rewrote all 30 001 files. The writer now keeps a
byte-exact cache of what it last published:

- on a cache hit with identical bytes, the entry is skipped (counted as
  `session_skipped_entries`);
- on first sighting in a fresh process, the entry is compared against the files
  on disk (they are warm in the page cache right after restore), so a restart
  does not rewrite a session either;
- a failed publish evicts the entry instead of leaving "this is current" behind;
- `erase_entry` evicts it too.

No hashing and no collision reasoning: the streams are compared as bytes. One
average entry is a few hundred bytes, so a 14k-torrent session keeps a few
megabytes in memory.

| Metric | Baseline | After |
|---|---|---|
| **1k** `shutdown.exit_s` (idle) | 6.63 s | **0.114 s** |
| **10k** `shutdown.exit_s` (idle) | 84.53 s | **0.517 s** |
| **10k** `startup-pty.exit_s` (Ctrl+C) | 80.77 s | **3.07 s** |
| 10k `session_save` | 30 001 ms (timeout) + 54 s lock wait | **243 ms**, no lock wait |
| 10k `files_rewritten` | 30 001 | **1** (`dht.cache`) |
| 10k `proc_write_bytes` | 122.9 MB | **4 KB** |
| 10k `session_skipped_entries` | — | **10 000 / 10 000** |

Shutting down *while* a mass verification is finishing writes real results
instead of nothing; that case is still bounded:

| 10k | idle | rechecks finishing |
|---|---|---|
| `session_save` | 243 ms | 588 ms |
| `files_written` | 0 | 245 |
| `shutdown.exit_s` | 0.517 s | 0.866 s |

`DISK_FLUSH_TIMEOUT` was also reduced from 35 s to 5 s. A flush is best-effort
(pieces are already synced per hash pass); with the old value a shutdown during
heavy hashing sat on that single stage for exactly 35 s before giving up.

## Watch-directory startup

`load_watch_directory` used to end with a synchronous full session commit. At
10 000 imported torrents that is 30 000 files written **before the TUI starts**:

| 10k, `-d files -w watch` | Before | After |
|---|---|---|
| `startup.total_ms` | **56 137 ms** | **824 ms** |
| `watch_scan` | 77 ms | 77 ms |
| `watch_import` | 745 ms | 740 ms |
| `watch_commit` (synchronous save) | **55 299 ms** | removed |

Imports are now persisted off the startup path: the periodic checkpoint writes
the static anchor (a `stat` for torrents that already have one) and the resume
data, and shutdown writes whatever is still pending. The watch directory remains
the source of truth, so a crash between import and the first checkpoint
re-imports from it on the next start.

The first shutdown after importing 10 000 torrents is the one-time cost of
persisting them, and it happens while 10 000 verifications are running:

| Phase | Time |
|---|---|
| `flush_downloads` (5 s cap) | 5 005 ms |
| `disk_thread` | 7 568 ms |
| `session_save` (12 485 entries, 2 files each) | 23 196 ms |
| total | 35.8 s |

That is real work — 10 000 torrents did not exist in the session before — and it
is bounded (`flush_downloads` 5 s, `session_save` 30 s with an abort flag).
Every later shutdown is the steady-state case below.


## Acceptance

| Criterion | Result |
|---|---|
| Ctrl+C → exit < 5 s at 10k | 0.52 s idle, 0.87 s during recheck, 3.07 s through the TUI |
| hard cap 10 s | every measured run is under it; the 30 s save timeout is not reachable in practice |
| unchanged session writes 0 files | 1 file (`dht.cache`), 4 KB |
| trusted resume queues 0 rechecks | proven by `test_restore_recheck` |
| lock released only after a consistent write | `lock_release` = 0 ms; the writer never holds the session mutex |

