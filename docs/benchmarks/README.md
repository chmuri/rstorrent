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
```

Results land in `docs/benchmarks/<timestamp>/`:

| File | Contents |
|---|---|
| `host.txt` | kernel, CPU count, `ulimit -n`, filesystem type, binary version |
| `startup.json` | `socket_s`, `rpc_s`, `full_s`, `rows`, peak RSS/threads/FDs, CPU |
| `lookup.json` | `d.hash` p50/p95/p99/max, hits/faults, QPS |
| `multicall.json` | `d.multicall2` p50/p95/p99, rows per second |
| `recheck.json` | `wall_s`, `mb_per_s`, peak concurrent checks, peak threads |
| `recheck-startup.json` | startup of the payload session (recheck queued) |

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
