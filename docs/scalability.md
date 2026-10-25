# Scalability model

How rstorrent behaves at 14,000+ torrents and hundreds of concurrent
rechecks, and which knobs change that. Numbers live in
[`docs/benchmarks/`](benchmarks/); this file explains the design.

## Status at a glance

| Area | Before | Now |
|---|---|---|
| Restore 14k paused torrents | 157.9 s (quadratic) | ~0.9–1.4 s (linear) |
| Graceful shutdown, 10k torrents | 84.5 s (30 s timeout + 54 s lock wait) | ~15–17 s, lock free during the write |
| Session files rewritten on an unchanged shutdown | every entry, 3 files each | every entry (dirty-only still pending) |
| `d.hash` p99 at 14k | 1.04 ms (scan) | 0.17 ms (flat in N) |
| `d.multicall2` at 14k x 12 | 47 s | ~0.18–0.22 s |
| Rechecks | one OS thread each | bounded pool, FIFO queue |
| Disk I/O | one global worker | sharded pool, per-torrent FIFO |
| Hash pass flushes | `fsync` per piece | one flush per pass |

## Thread model

```
Tokio runtime        SCGI/XML-RPC/JSON-RPC, timers, network supervision
hash scheduler       `storage.hash_concurrency` threads (default min(4, cores))
                     each runs one whole hash pass; queue is FIFO, unbounded
disk pool            `storage.disk_threads` workers (default 1)
                     every read/write/sync for a torrent goes to one shard
session loader       `storage.load_threads` scoped threads (default min(4, cores))
                     only alive during startup
```

Three invariants keep this safe:

1. **One torrent, one disk worker.** The shard is `u32::from_be_bytes(hash[0..4])
   % workers`, captured when `EngineDownload` is constructed. Writes, reads,
   syncs and the hash-pass flush for that torrent all use it, so per-torrent
   FIFO ordering is preserved. A torrent is never split across workers.
   Changing `storage.disk_threads` remaps torrents, which is safe because
   ordering is only required per torrent and remapping happens at startup.
2. **The pool must exist before the first download.** `init_global_disk_pool`
   runs before session restore. Downloads constructed earlier would capture a
   lazily created default pool and ignore later configuration.
3. **No blocking file I/O on the runtime.** Session parsing uses scoped OS
   threads; hash passes use the scheduler; disk work uses the pool. Handlers
   only submit commands and return.

## Lookup indexes

`DownloadList` and each `View` keep a `HashMap<HashString, usize>` beside their
`Vec`, so `find`, `insert` and duplicate rejection are O(1) while insertion
order (and therefore view ordering) is preserved. `erase` still reindexes the
tail, which is O(n) and acceptable because erases are rare.

Both indexes require `Download::info_hash()` to be immutable. `apply_metadata`
enforces that: metadata whose info hash differs from the download's is rejected
rather than silently re-keying the entry out of both indexes and the session
directory.

`d.multicall2` and `d.multicall.filtered` parse each column once and dispatch
the resolved command per row; empty arguments skip per-target `$` expansion.
Nested commands inherit the caller's trust level, so an untrusted connection
cannot escalate through a multicall wrapper.

## Session restore

`session::restore_entries` parses torrent, sidecar and resume data on a bounded
thread pool, returns results keyed by original index, and the caller inserts
them in that order. A corrupt entry or a panicking worker becomes a per-entry
error and is skipped; startup never aborts. Activation of previously-active
torrents happens after insertion, and queued rechecks are bounded by the hash
scheduler instead of spawning a thread per torrent.

## Hash passes

`prepare_recheck` flushes the disk worker once and `verification_owns_data()`
keeps new writes for that torrent out for the whole pass, while `HashTorrent`
reads bypass the worker. The pass therefore issues one flush before its first
read rather than one per piece. `HashTorrent` remains usable directly; a caller
that installs a `DiskThread` must keep new writes for that torrent out until the
pass finishes.

## Memory

Measured RSS at 14k paused torrents is ~140 MB (~10 KB/torrent), well under the
2.5 GB budget. The metadata-sharing refactor proposed in the plan is therefore
**deferred**: it is gated on a profile (heaptrack) showing a real cost, and no
such cost appears at this scale. Re-evaluate it if a many-piece workload shows
`pieces` vectors dominating RSS.

## Tunables

All are read at startup; changing them requires a restart.

| Command | Default | Range | Effect |
|---|---:|---:|---|
| `storage.disk_threads` | 1 | 1–64 | Disk workers. Raise for concurrent I/O across torrents. |
| `storage.max_open_files` | 64 | 4–65536 | Per-worker open-file cache. |
| `storage.hash_concurrency` | min(4, cores) | 1–1024 | Concurrent hash passes. Bounds CPU and read concurrency. |
| `storage.load_threads` | min(4, cores) | 1–64 | Session parse threads during startup. |
| `network.max_open_files` | 500 | 4–65536 | Alias that now feeds the same per-worker budget. |

### Descriptor budget

```
RLIMIT_NOFILE  >=  threads x max_open_files
                  + threads x (files open during a hash pass)   # reads bypass the cache
                  + sockets (connections, DHT, trackers)
                  + session files during commit
```

Startup logs a warning when `threads x max_open_files` exceeds a third of the
soft limit. The hash-pass term is the one the pool config does not cover:
`HashTorrent` opens its files outside the worker cache.

## Known limits

- **FUSE / `shfs` is a single bottleneck.** Worker count cannot scale past the
  union mount's serialisation. Benchmark FUSE and a direct mount separately.
- **Hash throughput follows `storage.hash_concurrency`, not
  `storage.disk_threads`.** Hash reads bypass the disk worker by design; the
  disk pool helps writes, reads, syncs and concurrent torrents.
- **`disk_threads` defaults to 1** to preserve existing behaviour. Raising it is
  an explicit opt-in.
- **Scale tests are `#[ignore]`d** and driven by `scripts/bench/run_all.sh`; CI
  never depends on wall-clock numbers.

## Validation

```bash
cargo test --workspace --all-targets          # incl. index, scheduler and pool invariants
scripts/bench/run_all.sh --count 14000        # startup, lookup, multicall
scripts/bench/run_all.sh --count 1000 --recheck-count 100 --recheck-mb 32 --threads 4
cargo test -p rt-core --test test_session_restore_parallel -- --ignored
cargo test -p lt-storage --test test_disk_pool_throughput -- --ignored --nocapture
```

The data-safety gate before enabling `storage.disk_threads > 1` in production is
the bitfield-equality test in `lt-storage/tests/test_disk_pool_bitfield_equality.rs`
plus the per-shard write tests in `test_disk_pool.rs`.
