# Benchmarks

Acceptance evidence for the 14k-torrent scalability work. Numbers from this
directory are the gate for every phase in
`docs/scalability.md`; a phase that cannot move its metric does not ship.

## Running

```bash
# full suite: 14k startup, 10k lookups, 20 multicalls
scripts/bench/run_all.sh

# fast sanity run on a laptop
scripts/bench/run_all.sh --count 1000 --lookups 500 --multicall 3 --no-build

# recheck concurrency (threads, MB/s) on a separate payload session
scripts/bench/run_all.sh --count 1000 --recheck-count 100 --recheck-mb 32

# shutdown latency, lock release and session rewrites
scripts/bench/run_all.sh --count 1000 --no-build --shutdown

# interactive TUI time-to-first-frame
scripts/bench/run_all.sh --count 1000 --no-build --pty

# restart over existing payload with resume data (the slow-restart case)
scripts/bench/run_all.sh --count 1000 --no-build --payload --shutdown
```

Results land in `docs/benchmarks/<timestamp>/`:

| File | Contents |
|---|---|
| `host.txt` | kernel, CPU count, `ulimit -n`, filesystem type, binary version |
| `startup.json` | `socket_s`, `rpc_s`, `full_s`, `rows`, peak RSS/threads/FDs, CPU, `metrics` |
| `startup-pty.json` | `first_frame_s`, `bytes_to_frame`, `exit_s` (interactive TUI on a 120x40 PTY) |
| `startup-warm.json` | `full_s` + `metrics` for a restart with resume data and payload present |
| `startup-payload-cold.json` | first start over existing payload (no resume): queues rechecks |
| `shutdown.json` | `exit_s`, `lock_free_s`, `files_rewritten`, `files_added`, `bytes_on_disk_delta`, `proc_write_bytes`, `shutdown_phases_ms` |
| `shutdown-warm.json` | the same measurements after the payload session was saved once |
| `lookup.json` | `d.hash` p50/p95/p99/max, hits/faults, QPS |
| `multicall.json` | `d.multicall2` p50/p95/p99, rows per second |
| `recheck.json` | `wall_s`, `mb_per_s`, peak concurrent checks, peak threads |
| `recheck-startup.json` | startup of the payload session (recheck queued) |

`metrics` comes from `RSTORRENT_METRICS_FILE` (one JSON object per line, see
`crates/rstorrent/src/metrics.rs`). It carries per-phase times and the session
writer counters, which is what separates "restore is slow" from "we rewrote
30,000 files".

## The metrics channel

`RSTORRENT_METRICS_FILE=/path/metrics.jsonl` enables it for one process. Events:

| Event | Meaning |
|---|---|
| `startup.phase` | one startup stage (`views`, `commands`, `config`, `session_lock`, `scan_entries`, `restore_entries`, `insert_downloads`, `cli_files`, `watch_scan`, `watch_import`, `watch_commit`, `network_spawn`) |
| `startup.summary` | totals: `torrents`, `rechecks_queued`, session-writer counters, `coordinators_spawned` |
| `shutdown.phase` | `network_drain`, `flush_downloads`, `disk_thread`, `session_save`, `lock_release`, with `completed`/`timed_out` flags |
| `shutdown.summary` | `total_ms`, `/proc/self/io` write volume, full session-writer counters |
| `exit.summary` | `wall_ms`, `interactive`, `forced` |

The session-writer counters are cumulative since process start, so the delta
between `startup.summary` and `shutdown.summary` is exactly the work done while
running. `session_staged_files` counts `.new` files, `session_fdatasync_calls`
counts per-file syncs and `session_dir_sync_calls` counts directory syncs.


Generated sessions live under `${TMPDIR:-/tmp/opencode}/rstorrent-bench/<stamp>`
and are deliberately **not** committed.

## What each metric proves

- **`startup.full_s`** — spawn to every expected row present in `d.multicall2`.
  This crosses session restore, `DownloadList` insertion and the 11 view
  mirrors. The quadratic term is visible by comparing 1k/5k/14k.
- **`lookup.p99_ms`** — `d.hash` by hash. Flat with N means the index works;
  growing with N means a scan is still in the path.
- **`multicall.p95_ms`** — the Flood poll. Scales with `rows x columns`; the
  per-command parse cost shows up here.
- **`recheck.mb_per_s` / `peak_threads`** — hashing throughput and the
  "one OS thread per recheck" behaviour. `peak_threads` should stop tracking
  `torrents` once the bounded scheduler lands.

## Budgets

Fill these from the first baseline run, then treat them as the pass/fail gate.
Do not invent numbers before measuring.

| Metric | 1k | 5k | 14k | Rule |
|---|---|---|---|---|
| `startup.full_s` | | | | ≤ 20x from 1k to 14k (linear-ish) |
| `startup.peak.rss_kb` | | | | ≤ 2.5 GB at 14k |
| `startup.peak.fds` | | | | ≤ 50% of `ulimit -n` |
| `lookup.p99_ms` | | | | within 1.5x of the 1k value |
| `multicall.p95_ms` | | | | within 1.5x of the 1k value per row |
| `recheck.mb_per_s` | | | | at NVMe line rate for the piece size |
| `recheck.peak_threads` | | | | ≤ configured `storage.hash_concurrency` + baseline |
| `startup-pty.first_frame_s` | | | | ≈ `startup.full_s`; UI must not wait behind restore |
| `shutdown.exit_s` | | | | < 5 s at 1k and 10k, hard cap 10 s |
| `shutdown.files_rewritten` | | | | 0 when the session did not change |
| `startup.rechecks_queued` (warm) | | | | 0 when the resume data is trusted |

`startup.peak.threads` is informational only: thread count is expected to rise
with the number of active rechecks until the bounded scheduler exists.

## Caveats

- **FUSE/`shfs` is a single bottleneck.** Run the benchmark twice — once on the
  union/FUSE mount, once on a direct mount — and never claim linear scaling on
  FUSE. Record the filesystem type from `host.txt`.
- Sparse payloads (`gen_payload.py` without `--fill`) read as zeros. They are
  fast on some filesystems; use `--fill` before quoting absolute MB/s.
- These scripts drive a real daemon over its Unix socket. They are not part of
  `cargo test`; CI must not depend on wall-clock numbers.
