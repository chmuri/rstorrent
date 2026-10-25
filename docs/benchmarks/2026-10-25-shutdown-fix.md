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

## Before → after

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

## What is still slow

At 10 000 torrents shutdown is ~16 s, not the 5 s target. The remaining cost is
entirely the number of files written: 30 001 unchanged session files, each with
its own `fsync`. Parallelism has taken what it can; the next step has to stop
writing files that did not change:

| Remaining option | Files written at 10k | Expected `exit_s` |
|---|---|---|
| skip unchanged entries (content/dirty tracking) | ~0 | < 1 s |
| skip `.torrent` rewrites when the file exists | 20 000 | ~11 s |
| current | 30 001 | ~16 s |

`files_rewritten` and `proc_write_bytes` are in the benchmark output
specifically so that change is accepted only when those two numbers move.
