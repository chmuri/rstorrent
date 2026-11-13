# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.21.0] - 2026-11-14

### Added
- **BitTorrent v2 / hybrid (BEP 52)**:
  - Direct `sha2` dependency and BEP 52 Merkle trees in `lt-crypto::merkle`
    (16 KiB leaves, zero-hash padding, per-file `piece_root` / `piece_layers`),
    verified 1:1 against the reference `bep_0052_torrent_creator.py`.
  - v2 / hybrid `info_dict` parser (`piece_layers`, `piece_root`, `file tree`)
    with dual identity hashing (v1 SHA-1, v2 SHA-256 truncated to 20 bytes).
  - New `Hash256` type alongside the 20-byte wire `HashString`; magnet
    `xt=urn:btmh:` multihash decoding and `xs=` exact-source support.
  - Piece-aligned v2 storage layout, per-file resume bitfields, and Merkle-proof
    block verification on both the receive and recheck paths.
- **Native WebUI** (`crates/rstorrent/src/webui/`): axum REST API mapped onto the
  `CommandMap`, WebSocket live events/stats bridge, bearer-token auth with
  server-side trust, optional rustls TLS, and a self-contained embedded SPA
  (no external CDN). Opt-in via `[webui]` / `--webui-addr`.
- **Global and per-torrent rate limiting**: token-bucket `Throttle` enforced on
  the peer read/write path (global + named levels), driven by a 100 ms refill
  loop; `d.throttle_name` maps a torrent to a shared named throttle group.
- **Stop-on-ratio**: production `ratio_reached` evaluation in the choking
  supervisor executes `ratio.command` and stops seeding once the group ratio is
  reached (collect-then-act, off the engine lock).
- **Global connection budget**: process-wide `ConnectionBudget` with inbound cap
  sourced from `max_connections`, a global outbound-dial semaphore, and a global
  peer-slot limit covering inbound and outbound sessions of every torrent.
- **WebSeed (BEP 19 / BEP 17)**: HTTP range consumer for `web_seeds` with SHA-1
  verification, used as a piece source when peers are unavailable.
- **uTP / LEDBAT dial + accept**: outbound uTP path alongside TCP and inbound
  `UtpSocket` accept loop wired into the peer session runtime.
- **DHT hardening**: IPv6 (`nodes6` / `peers6`), time-rotated token secret,
  multi-bucket `get_closest_nodes`, `implied_port`, and BEP 51 `sample_infohashes`.
- **Port mapping**: lease renewal, UDP mapping, shutdown cleanup, and NAT-PMP
  gateway discovery replacing hardcoded addresses.
- **Proxy & DNS**: SOCKS5 / HTTP proxy and cached DNS resolver wired into dialing.
- **BEP 6 producer** (`AllowedFast` / `SuggestPiece`) and **LTEP** `yourip` /
  `ipv4` / `ipv6` handshake extensions.
- **Prometheus metrics**: opt-in `/metrics` endpoint (`--metrics-addr` /
  `[metrics]`) exporting session, throttle, DHT, SCGI, peer and per-torrent stats.
- **Lua scripting in the daemon**: `register_lua_commands` wired into
  `Control::initialize`, loading `rtorrent.lua` from the config directory.
- **TUI client parity**: mouse support, theme picker, command palette, and a Logs
  tab ported from the in-process TUI.
- **Quality infrastructure**: `cargo-fuzz` targets (bencode, message, magnet, DHT
  KRPC, uTP, torrent/resume) with CI smoke + nightly runs; criterion hot-path
  benchmarks with CI regression gating; loom lock-order models and proptest
  parser/selector properties; `cargo-llvm-cov` coverage job; `[workspace.lints]`
  with `deny(unsafe_code)` for the nine unsafe-free crates.
- **Generic `d.custom` persistence** and **`scheduler.max_active`** enforcement at
  the activation chokepoints (`start` / `try_start`).

### Changed
- **Tit-for-tat choking**: rate-based ranking with snubbed peers deprioritized and
  a random optimistic unchoke every third round (replacing deterministic rotation).
- **Zero-copy data path**: piece assembly uses `BytesMut`/`Bytes` end-to-end from
  the block pool to the disk queue, removing the per-piece `to_vec` copy.
- **Write-through piece cache** (fresh uploads served from RAM) and an LRU-bounded
  `ConcurrentReadCache` replacing unbounded file-handle growth.
- **Word-level rarity**: `u64` CLZ accounting with `u16` counts, lifting the legacy
  255 saturation cap to 65535.
- **Configurable logging**: `EnvFilter` (`RUST_LOG`) plus a config `LevelFilter`,
  installed in both daemon and interactive modes; `log.execute/rpc/xmlrpc` now
  open real file outputs.
- **TOML configuration applied end-to-end**: `enable_utp` / `enable_upnp` flags
  gate the uTP accept loop and port-mapping supervisor; error logging replaces
  silent `let _ =` command application.
- **README synchronized with the code**: 16 crates, 10 `TorrentService` gRPC
  methods, 6 themes, and real TOML keys.

### Fixed
- **Reactor stalls**: received-piece SHA-1/SHA-256 verification moved off the
  reactor into `spawn_blocking`; blocking pause `fsync` moved off the engine lock
  via a snapshot + `spawn_blocking` (disk-queue backpressure already async).
- **LEDBAT one-way-delay measurement** now uses a monotonic clock instead of time
  since last receive.
- **v2 `ut_metadata` verification**: pure-v2 torrents (truncated SHA-256 identity)
  previously failed SHA-1-only verification; now verified with SHA-256 when the
  full `btmh` hash is present.
- **Domain error hierarchy**: `SessionError` (rt-core) and `SysError` (lt-sys)
  replace stringly-typed errors at the session/watch/scheduler boundaries.
- **DNS resolver startup panic** (`resolver.rs`) replaced with a graceful fallback.

### Security
- **gRPC hardening**: rustls TLS, constant-time bearer-token auth, default
  loopback bind, and server-side trust derivation (the client-supplied `trusted`
  flag is ignored).
- **SCGI hardening**: UNIX socket `0o600` and TCP authentication.
- **Removed hardcoded sccache S3 credentials** from the build scripts; they are
  now injected via the environment or a gitignored local config
  (`scripts/remote-ci.local.env`), and purged from all git history.

## [0.20.1] - 2026-11-06

### Added
- **Persistent numbered custom fields**: `d.custom1`..`d.custom5` now live on the
  download and are written to the `.rtorrent` session sidecar as
  `custom1`..`custom5`, so labels set by an external client survive a daemon
  restart.
- **gRPC `TorrentSummary.category`**: the native `rtorrent.v1.TorrentService`
  now reports the torrent label (`d.custom1`) on every summary, which lets
  download clients such as Sonarr filter and tag their own torrents.

### Fixed
- `d.custom1`..`d.custom5` no longer disappear when the command map is rebuilt
  (e.g. after a configuration reload) and are consistently scoped per download.

## [0.20.0] - 2026-11-03

### Added
- **BEP 29 Micro Transport Protocol (uTP) & LEDBAT Congestion Control**:
  - Complete framing and parsing for 20-byte uTP packets (`ST_DATA`, `ST_FIN`, `ST_STATE`, `ST_RESET`, `ST_SYN`) with Selective ACK (SACK) extension in `crates/lt-net/src/utp/`.
  - Full LEDBAT congestion control with rolling base-delay minimum filter, 100 ms target queuing delay, dynamic window sizing, and RFC 6298 Jacobson-Karels retransmission timeouts.
  - `UtpStream` implementing Tokio `AsyncRead` and `AsyncWrite` with out-of-order packet reassembly and in-flight retransmissions.
  - `UtpSocket` multiplexing multiple concurrent streams over a single `tokio::net::UdpSocket`.
  - Seamless inbound uTP peer session handling in `lt-engine` alongside TCP.
- **Modern `rstorrent.toml` Configuration Format**:
  - Modern TOML configuration schema (`crates/rstorrent/src/toml_config.rs`) covering `[network]`, `[storage]`, `[protocol]`, `[rpc]`, and `[dht]`.
  - Standard XDG configuration resolution searching `./rstorrent.toml`, `./.rstorrent.toml`, `$XDG_CONFIG_HOME/rstorrent/rstorrent.toml`, `~/.config/rstorrent/rstorrent.toml`, and `~/.rstorrent.toml` before falling back to legacy `.rtorrent.rc`.
- **Automatic UPnP IGD & NAT-PMP Port Forwarding**:
  - Autonomous SSDP M-SEARCH discovery and SOAP `AddPortMapping` client in `crates/lt-net/src/port_mapper.rs`.
  - Background mapping supervisor in `rt-core` automatically mapping incoming TCP and UDP/uTP listen ports on home routers.
- **Real-Time Watch Directory Monitoring**:
  - Event-driven filesystem monitoring using `notify` (inotify on Linux) in `crates/rstorrent/src/control.rs`.
  - Newly added `.torrent` files are automatically ingested, committed to `DownloadList` and `ViewManager`, and started immediately.
- **Tit-for-Tat Choking Supervisor**:
  - Autonomous Tit-for-Tat choking supervisor loop in `crates/rt-core/src/network_runtime.rs` running 10-second evaluation rounds.
  - Prioritizes fastest uploaders during leeching and highest downloaders during seeding, with 30-second optimistic unchoke rotation.
- **Endgame Mode Broadcast CANCEL**:
  - Global peer sender registry in `lt-engine` broadcasting `CANCEL` messages across all active peer connections upon block completion.

### Changed
- **Asynchronous Non-Blocking Disk I/O Pipeline**:
  - Decoupled peer upload block requests from the main `Download` state mutex via `read_piece_block_or_async_params`.
  - Uncached block reads are offloaded directly to background disk threads without locking download state or starving concurrent peers.
- **TUI & CLI Stubs Completed**:
  - Replaced hardcoded disk capacity with live POSIX `statvfs` available and total filesystem metrics in `crates/rt-tui/src/concurrency/aggregator.rs`.
  - Implemented interactive priority controls (`+`/`-`) for both torrents and individual files inside the inspector drawer.
  - Connected throttle adjustment hotkeys (`a`/`z` for upload limit, `A`/`Z` for download limit).
  - Bound CLI flags `-b` (`bind_address`) and `-i` (`local_address`) into command mapping and network configuration.
  - Populated complete file lists and peer connections in gRPC `rtorrent.v1` service endpoints.

## [0.19.4] - 2026-11-03

### Added
- **In-memory thread-safe DNS caching for trackers**:
  - Implemented 5-minute TTL DNS resolution caching (`tracker_dns_cache`) for tracker endpoints in `lt-tracker`.
  - Eliminates glibc `getaddrinfo` blocking lockups and scales announce requests across large swarms (reducing 40,000 DNS resolutions to 4 when endpoints match).
- **Tracker and DHT concurrency throttling**:
  - Added concurrency semaphore (`tracker_concurrency_semaphore`) bounding concurrent HTTP and UDP tracker announces/scrapes to 32 parallel requests.
  - Added DHT lookup semaphore (`dht_lookup_semaphore`) in `lt-dht` bounding simultaneous DHT searches to 32 concurrent requests.
- **Direct slice accessors in DownloadList**:
  - Added `DownloadList::downloads()` returning `&[Arc<RwLock<Download>>]` slice references, eliminating expensive cloning of 10,000 `Arc` handles on every UI sampling tick.
- **Unified single-pass UI state aggregation**:
  - Implemented `sample_engine_snapshot_with_stats` in `rt-tui`, consolidating category counting, transfer rate calculation, and snapshot generation in a single atomic iteration pass.

### Fixed
- **UI freeze and thread exhaustion with 10,000+ torrents**:
  - Fixed Tokio blocking thread pool saturation where 10k torrents starting simultaneously spawned 512 blocking threads (>5.8 GB VSZ) trying to resolve DNS queries.
  - Throttled TUI engine snapshot sampling to 4 Hz (250 ms) instead of 60 Hz, decoupling screen redraws via dirty flags (`needs_draw`) and allowing the event loop to idle on poll.
  - Eliminated UI thread starvation caused by allocating 40,000 heap Strings and running Rayon multi-threaded sorts 60 times/second.
  - Reduced peak thread count from 537 to 34–58 threads, VSZ from 5.8 GB to 2.2 GB, and stabilized RSS memory at ~195 MB under 10,000 torrent load with instant keyboard responsiveness.
- **Coordinator shutdown stall**:
  - Added `has_pending_disk_writes` check to skip blocking disk flush workers during download shutdown when no writes are pending.

### Changed
- **Remote CI Builder Performance**:
  - Set remote Docker builder default jobs to `auto` (saturating all host CPU cores) and memory limit to 4 GB (`MEMORY=4g`).

## [0.19.3] - 2026-11-03

### Changed
- **Renamed title display to `rstorrent`**: Updated Dashboard telemetry header and startup status messages from `rtorrent-next` to `rstorrent v0.19.3`.

### Fixed
- **Full mouse and keyboard interactivity for Category View and Torrent Inspector tabs**:
  - Implemented spatial hit-testing for Inspector Drawer tabs (`[1:General]`, `[2:Trackers]`, `[3:Peers]`, `[4:Files]`, `[★ 5:Piece Map]`, `[6:Logs]`) and close button `[i: Close]`, enabling instantaneous mouse switching and drawer toggling.
  - Implemented content body hit-testing to click-focus the inspector and select item rows directly.
  - Refined Category View tab spatial hit-testing to eliminate dead zones between tabs and on separators.
  - Added direct keyboard number jumping (`1`..`6`) when inspector is focused to jump directly to any inspector tab.
  - Added `Tab` / `Shift+Tab` and `Left`/`Right` / `h`/`l` inspector tab cycling.
  - Added `Left`/`Right` / `h`/`l` view cycling in Normal mode when table is focused.
  - Added `Tab` shortcut in Normal mode to open and focus the Inspector drawer.
  - Fixed selection cursor synchronization in `InspectorFocus` mode so `CursorDown` / `CursorUp` updates `inspector_sub_idx` properly.
  - Populated live detailed diagnostics (files, peers, trackers, piece verification heatmap, and save path / status message) for the focused torrent upon opening the inspector.

## [0.19.2] - 2026-11-02

### Added

- **Multi-threaded parallel bulk torrent deletion & teardown**:
  - Accelerated batch torrent deletion (`DownloadList::erase_batch`) across all CPU cores using Rayon parallel iterators (`par_iter`), dropping teardown time for thousands of torrents from minutes to milliseconds.
  - Added parallel batch deletion of on-disk session files (`SessionManager::erase_entries_batch`), executing `.torrent`, `.libtorrent_resume`, and state file unlinks concurrently across threads.
  - Added batch event listener mechanism (`event.download.erased_batch`) to `DownloadList`, synchronizing view removals and session cleanup in a single batched pass.
  - Background asynchronous session unlinking in `control.rs` via `tokio::task::spawn_blocking` to prevent blocking the engine event loop during large batch deletions.
  - Scale integration test `test_bulk_erase_scale` benchmarking erasure of 2,500 out of 5,000 torrents across 11 views in ~125 ms.

### Fixed

- **$O(N^2)$ algorithm bottleneck & UI freeze during bulk deletion**:
  - Replaced $O(N^2)$ sequential element removal (`downloads.remove(pos)` and linear reindexing) in `DownloadList` and `View` with $O(N)$ single-pass partition draining (`drain(..)`) and single-pass hash-to-index reconstruction.
  - Eliminated redundant per-torrent focus recalculations and view change notifications (`emit_changed()`), clamping focus once and notifying listeners once per batch.
  - Updated native TUI (`rt-tui`) to execute batch deletion (`erase_target_downloads`) when erasing selected torrents via `^D` or the context menu (`d.erase=`), preventing main thread lockups on large selections (e.g. 14,000 torrents).
  - Updated `rt-grpc` `bulk_remove_torrents` to utilize `erase_batch` and `erase_entries_batch`.

## [0.19.1] - 2026-11-02

### Added

- **Bulk operations & selection in native TUI (`rt-tui`)**:
  - Implemented multi-selection in `RootUI`: `Space` toggles focus item selection, `Ctrl+A` selects all downloads in the view, `*` inverts selection, and `Esc` clears selection.
  - Multi-torrent shortcuts: `^S` (start), `^D` / `x` (stop active / erase stopped), `^K` (stop, close, ignore commands), `^R` (re-check hash), and `+`/`-` (priority adjustment) now operate over all selected torrents.
  - Bulk context menu: pressing `m` with multiple torrents selected opens `Download (N selected)` context menu, dispatching commands across all selected items.
  - Visual indicators: selected rows are highlighted in yellow with `*` prefix (`>` indicates focus when selections are active); status bar displays `[N selected]`.

### Fixed

- **Key modifier handling in `rt-tui-client`**:
  - Fixed `*` (invert selection) requiring `KeyModifiers::NONE`, allowing it to trigger with `Shift` on standard keyboard layouts.
  - Fixed Ctrl shortcuts (`^A`, `^S`, `^D`, `^K`, `^R`) to be case-insensitive for both lowercase and uppercase key codes.

## [0.19.0] - 2026-11-01

### Added

- **Multi-core hash checking & Rayon acceleration**:
  - Replaced single-threaded sequential piece verification in `lt-storage` and `lt-engine` with Rayon parallel iteration (`par_iter()`), utilizing all CPU cores and saturating NVMe SSD read bandwidth.
  - Implemented `ConcurrentReadCache` using thread-safe stateless `pread(2)` file handle sharing without locks across worker threads.
  - Scaled default `DiskPool` worker threads to automatically adapt to `available_parallelism() / 2` (minimum 2, default up to 16 threads).
- **In-memory LRU read cache (`PieceCache`) & I/O optimization**:
  - Implemented `PieceCache` in `lt-storage`: in-memory LRU cache serving upload and recheck requests directly from RAM without disk I/O.
  - Added Linux `posix_fadvise(POSIX_FADV_DONTNEED)` cache eviction advice on completed piece writes to prevent OS page cache pollution.
  - Optimized network framing in `lt-engine`: eliminated heap vector clone (`buf.to_vec()`) on every outgoing message batch, performing direct in-place encryption on `BytesMut`.
  - Consolidated `Download` mutex acquisitions in `network.rs` when receiving piece blocks, eliminating lock contention across peers.
- **Protocol enhancements (LSD, UPnP / NAT-PMP, WebSeeds)**:
  - Local Peer Discovery (LSD - BEP 14) over UDP multicast `239.192.152.143:6771` with automatic LAN peer discovery and periodic announcements.
  - Added `PeerSource::Lsd` and strict private torrent enforcement (disabling LSD for private torrents).
  - Automatic Port Forwarding via NAT-PMP and UPnP IGD in `lt-net` and `rt-core`.
  - WebSeeds (BEP 19 GetRight & BEP 17 httpseeds) metadata parsing in `DownloadFactory` and URL storage in `Download`.

### Added

- **Bulk operations across TUI and gRPC**:
  - Multi-selection in TUI torrent list (`Space` toggles item, `Ctrl+A` selects all, `*` inverts selection, `Esc` clears selection).
  - Bulk shortcuts with visual indicator (`*` prefix and highlighted rows): `^S` (start), `^D` (stop / remove), `^K` (close), `^R` (re-check), `+`/`-` (bulk priority change).
  - Context menu bulk entries when multiple torrents are selected ("Start (N torrents)", "Stop (N torrents)", etc.).
  - Bulk file selection and priority cycling in details view Files tab (`x`/`v`/`Insert` to toggle, `Ctrl+A` to select all, `Space`/`*` to cycle priority across all selected files, `Esc` to clear selection).
  - New gRPC RPCs in `TorrentService`: `BulkControlTorrents`, `BulkRemoveTorrents`, and `BulkSetTorrentPriority`.
  - Added `[N selected]` indicator in the TUI status bar.
- **DHT integration and readiness**:
  - BEP 5 `PORT` message handling wired between peer network traffic and DHT routing table (`dht.add_peer_node`), enabling automated peer node discovery.
  - Magnet URI support in `rt-grpc` via `load_magnet`.
  - DHT status reporting (`dht_active`, `dht_nodes`, `dht_torrents`, `dht_port`) in `SessionStatsResponse` and TUI status bar.

## [0.17.2] - 2026-10-29

### Added

- **TUI view switching**: keys `1`..`9` and `0` switch views (`main`, `name`,
  `started`, `stopped`, `complete`, `incomplete`, `hashing`, `seeding`, `leeching`,
  `active`), matching classic rTorrent behavior and immediately triggering a state refresh.
- **TUI list filtering**: `/` and `f` open a filter prompt; active filters narrow the
  displayed torrents in real time, show in the title bar (`[<view>:<filter>]`), and
  can be dismissed with `Esc` or an empty prompt.
- **TUI navigation & shortcuts**:
  - `Enter` opens the details view for the selected torrent (alongside `Right` and `l`).
  - `Esc` and `Backspace` exit the details pane from any tab back to the torrent list.
  - Arrow keys and PageUp/PageDown navigate rows in the Peers and Trackers tabs.
  - `l` and `Esc` close the log view in addition to `Space` and `L`.

## [0.17.1] - 2026-10-29

### Fixed

- **Release packages now ship `rstorrent-tui`**: the release workflow only built
  `-p rstorrent`, so the standalone TUI client added in 0.17.0 was missing from
  the tarballs and `.deb` packages. `scripts/package.sh` takes an optional
  `TUI_BINARY` and installs it next to the daemon (including the macOS codesign
  pass), and the release workflow builds and packages both binaries.

## [0.17.0] - 2026-10-29

Standalone gRPC TUI client (`rstorrent-tui`), implemented end to end from
`docs/tui-implementation.plan.md` (TASK-01 … TASK-16).

### Added

- **`rt-tui-client` crate** — new workspace member producing the `rstorrent-tui`
  binary: clap CLI (`--endpoint`, `--theme`, `--refresh-ms`), typed endpoint
  parsing for `host:port`, `tcp://` and `unix://` with a bounded reachability
  probe.
- **gRPC bridge** (`GrpcBridge`) over TorrentService/SystemService/CommandService
  with a per-call timeout (default 5 s) and a four-variant `BridgeError`.
- **View model** (`AppState`) with display-ready torrents, session statistics,
  files, peers, trackers, details tabs, context menu, prompt, log and theme.
- **ratatui widgets**: download list (adaptive columns, progress bar, focus),
  title and status bars, details pane with Info/Files/Peers/Trackers/Chunks tabs,
  input prompt, context menu popup and log view.
- **Interaction**: vi-style navigation, torrent shortcuts (`^S`, `^D`, `^K`,
  `^R`, `+`/`-`, `I`), file priority commands via `f.priority.set`, command
  prompt (`command=1,2`), load prompt, and throttle keys for the qwerty, azerty,
  qwertz and dvorak layouts.
- **Real-time updates**: background `StreamEvents`/`StreamStats` subscribers over
  a bounded channel with incremental `apply_event`/`apply_stats`.
- **Connection health**: reconnection state machine with 1 s → 30 s backoff,
  10 s health checks, reconnection countdown and a stale-data marker.
- **Configuration**: `~/.config/rstorrent-tui/config.toml` (XDG-aware) with
  endpoint, refresh interval, throttle layout and theme colours.

### Notes

- 234 tests cover the client crate; `cargo llvm-cov -p rt-tui-client` reports
  86.97 % line coverage.

## [0.16.1] - 2026-10-26

Hotfix and runtime engine wiring release: fixes BDP throughput/RTT collapse, Rayon hashing synchronization, per-piece disk sync stalls, Bitfield word desynchronization, and finishes runtime integration of BlockPool, batching, speed classification, and endgame evaluation.

### Fixed

- **BDP Throughput & RTT Calculation**:
  - Replaced queue sojourn time in `RequestList::on_block_received` with inter-arrival delta calculation, preventing window collapse during pipelined block reception.
  - Scaled out queuing delay from `rtt_ewma` when inflight requests exceed 2, bounded by `min_rtt * 2`.
  - Wired snub detection `request_list.check_snubbing()` into the periodic peer tick.
- **Rayon Worker Hashing**:
  - Dispatched hash computations asynchronously using `rayon::scope(|s| s.spawn(...))` and parallelized multi-node batches via `into_par_iter()`, eliminating executor thread stalls.
- **Disk I/O IOPS Stall**:
  - Removed per-piece `f.sync_data()` call in `write_piece_to_disk`. Added standalone `sync_files` invoked on torrent completion, pause, and graceful shutdown.
- **Bitfield Synchronization**:
  - Rebuilt internal `u64` words from raw byte slices in `Bitfield::update` before tail truncation and bit counting.
- **ChokeQueue Truncation Overflow**:
  - Clamped `ratio` computation before u32 casting in `Heuristics::RateBasedBitTyrant`.

### Changed

- **Engine Wiring**:
  - Reordered `PeerSpeedClass` enum to `Slow < Medium < Fast` and wired dynamic speed tiering + endgame evaluation in `Download::delegate_for_peer`.
  - Optimized `ChunkSelector::search_bucketed_rarest` to perform 64-bit word-level intersections with `leading_zeros()`, eliminating cloning overhead on priority ranges.
  - Integrated `BlockPool` allocation for incoming piece buffers and coalesced outbound socket request writes with `PeerMessage::Batch` / `encode_batch`.

## [0.16.0] - 2026-10-26

Algorithmic and throughput release: state-of-the-art BitTorrent piece selection,
BDP dynamic request pipelining, rate-based choking (BitTyrant heuristics), lock-free
zero-copy buffer pools, multithreaded SHA-1 hashing, and positioned asynchronous disk I/O.

### Added

- **Bucketed Rarest-First Piece Picker with SIMD/u64 acceleration**:
  - `Bitfield` refactored to aligned `u64` bit slices with hardware `POPCNT` (`count_ones()`)
    and `CLZ` (`leading_zeros()`), speeding up interesting-piece queries by 64x ($O(N/64)$).
  - Strict priority tiers: Tier 0 (Partial Pieces drained first), Tier 1 (Header/Footer preview
    ranges for streaming and metadata container probing), Tier 2 (Bucketed rarest-first).
  - Random tie-breaking among rarest pieces sharing minimal availability to eliminate the
    *thundering herd* problem across swarms.
- **Piece Affinity & Dynamic Endgame**:
  - Peers classified into speed tiers (`Fast`, `Medium`, `Slow`) via EWMA download throughput.
  - Piece Affinity prevents mixing `Slow` and `Fast` peers on the same piece, eliminating the
    *last-block straggler* delay.
  - Dynamic Endgame mode activates when all pending blocks are in flight, duplicating requests
    only to top-tier `Fast` peers and firing `CANCEL` immediately upon block receipt.
- **BDP Dynamic Request Pipelining**:
  - Dynamically calculates inflight request queue depth $Q_{\text{target}} \in [4, 512]$
    matching the Bandwidth-Delay Product (BDP) with target buffer time $\tau = 1.5\text{ s}$.
  - EWMA rate ($\alpha = 0.2$) and RTT tracking for each peer connection.
  - Snub detection ($T_{\text{snub}} = \max(15\text{ s}, 4 \times \text{RTT}_{\text{ewma}})$)
    dropping hung peers to $Q_{\text{target}} = 2$ and returning unfulfilled blocks to the pool.
- **Lock-Free Buffer Pool (`BlockPool`) & Zero-Copy Batching**:
  - Lock-free 16 KiB block buffer arena based on `crossbeam_queue::ArrayQueue<BytesMut>`
    with `PooledBlock` RAII guards, eliminating allocator churn on high-speed connections.
  - Vectored batch framing (`encode_batch`) coalescing multiple 17-byte `REQUEST` messages
    into single socket flushes, reducing syscall frequency up to 50x.
  - Borrowed-slice zero-copy views in Bencode parsing (`as_raw_string_slice`, `as_raw_list_slice`,
    `as_raw_map_slice`).
- **Multithreaded CPU Hashing via Rayon**:
  - CPU-intensive piece SHA-1/SHA-256 integrity verification is offloaded to a Rayon threadpool
    via `rayon::scope`, preventing Tokio runtime executor starvation and connection drops.
- **Rate-Based Choking & BEP 6 (Fast Extension)**:
  - BitTyrant-inspired rate-based choking heuristics ($\eta_i = D_i / \max(U_i, 1)$) optimizing
    reciprocal upload yield.
  - Optimistic unchoke rotation with newcomer boost for discovering fast swarming peers.
  - Complete handling of BEP 6 fast extension messages: `RejectRequest`, `HaveAll`, `HaveNone`,
    `SuggestPiece`, and `AllowedFast`, releasing unserved blocks without waiting for timeouts.
- **Positioned Asynchronous Disk I/O & Piece Write Coalescing**:
  - Sequential `seek()` + `write_all()` replaced with atomic positioned I/O (`FileExt::write_all_at`
    on Unix, `seek_write` on Windows) and `read_exact_at`.
  - In-memory piece write coalescing buffering incoming unordered blocks before a single
    sequential disk commit, reducing disk IOPS by over 90%.

## [0.15.0] - 2026-10-26

Latency release: a restart no longer rehashes or rewrites the session, and
Ctrl+C is bounded by the work actually pending instead of by a fixed timeout.
Numbers are measured at 10,000 torrents in `docs/benchmarks/`.

### Added

- **Watch directory (`-w/--watch-dir`, alias `--watch`)**: every `.torrent`
  file directly inside the directory is imported and started at startup, in
  sorted order, case-insensitively on the extension. Malformed torrents and an
  unusable directory are logged and skipped without aborting startup.
- **Startup and shutdown metrics (`RSTORRENT_METRICS_FILE`)**: one JSON object
  per line with per-phase timings (`startup.phase`, `startup.summary`,
  `shutdown.phase`, `shutdown.summary`, `exit.summary`), session-writer
  counters (entries, staged files, `fdatasync` and directory syncs, bytes,
  skipped entries, commit calls) and the process write volume from
  `/proc/self/io`. Written by the process itself, so it also works in the TUI
  where no `tracing` subscriber is installed.
- **`-d/--directory` long form** (alias `--download-dir`) alongside the existing
  short flag.
- **Benchmark coverage for shutdown and the TUI**: `bench_shutdown.py`,
  `bench_start.py --pty`, `--metrics`, and `run_all.sh --payload --shutdown
  --pty`. Reports `exit_s`, `lock_free_s`, `files_rewritten`,
  `proc_write_bytes` and `first_frame_s`.

### Changed

- **Session writes no longer hold the session mutex**: `SessionManager::writer()`
  snapshots the directory and fsync policy, so `stop()` and shutdown never wait
  behind a multi-thousand-entry commit. Entries are published across a small
  worker pool; each entry still gets its own `fdatasync` and directory `fsync`.
- **Unchanged entries are not rewritten**: the writer compares the serialized
  streams byte-for-byte with the last published generation, and with the files
  on disk on the first sighting after a restart. An idle shutdown now rewrites
  one file (`dht.cache`) instead of 30,001.
- **Shutdown deadlines stop the writer** instead of abandoning it: a timed-out
  save sets an abort flag checked between entries, then waits a short grace
  period before the session lock is released.
- **Watch-directory imports are persisted off the startup path** by the periodic
  checkpoint and by shutdown. The watch directory remains the source of truth,
  so a crash before the first checkpoint re-imports it on the next start.
- **`session.checkpoint` uses the same writer** and writes the static `.torrent`
  anchor for torrents that do not have one yet.
- **`DISK_FLUSH_TIMEOUT` 35s → 5s**: the final flush is best-effort; pieces are
  already synced per hash pass.
- **Remote build/test wrapper**: `scripts/remote-ci.sh` runs
  `cargo build|check|test|clippy|fmt` on a Docker builder over SSH, and
  `.opencode/plugins/remote-ci/index.ts` routes agent shell calls through it so
  compilation does not compete with the workstation. See `docs/remote-ci.md`.
- **Metrics are opt-in** via `RSTORRENT_METRICS_FILE=/path/metrics.jsonl`.
  Watch-directory imports reach the session at the checkpoint interval
  (`session.save_interval`, 300s by default) and at shutdown; the first shutdown
  after importing 10,000 torrents is still a one-time bounded write (~36s),
  every later shutdown is the idle case.

### Fixed

- **Ctrl+C no longer hangs**: shutdown at 10,000 torrents dropped from 84.5s to
  0.52s idle, 0.87s with rechecks running and 3.07s through the TUI. The 30s
  session-save timeout no longer *adds* time to shutdown, and the 54s wait for
  the session lock is gone.
- **Trusted resume data is kept**: restore queued a recheck for every torrent
  regardless of a validated bitfield; now only entries without resume data, or
  with no completed pieces, verify the payload.
- **A verification in flight no longer persists an empty bitfield**: such a
  download writes only its static `.torrent`; resume data is left to the next
  completed pass.
- **Watch-directory startup**: 56.1s → 0.82s at 10,000 torrents (the
  synchronous session commit was removed from `initialize`).
- **The download list scrolls**: the viewport now follows the focused row, so
  every torrent is reachable with the arrow keys, Home/End, PageUp/PageDown and
  the mouse wheel. Focus no longer walks past the last torrent, and the counter
  never shows `N+1 of N`. PageUp/PageDown previously only worked in the other
  windows, and mouse events were read but ignored by the event loop.

## [0.14.0] - 2026-10-24

Scalability release: the daemon is now usable with 14,000+ torrents and
hundreds of concurrent rechecks.

### Added

- **Bounded hash scheduler (`rt-core`)**: rechecks run on a fixed pool
  (`storage.hash_concurrency`, default min(4, cores)) with a FIFO queue instead
  of one OS thread per pass. `d.check_hash` queues there instead of blocking the
  RPC thread.
- **Sharded disk worker pool (`lt-storage`)**: `storage.disk_threads` workers
  (default 1), assigned by info hash so a torrent's writes, reads, syncs and
  hash flush stay on one FIFO queue. Statistics aggregate across workers.
- **`storage.*` settings**: `disk_threads`, `max_open_files`, `hash_concurrency`
  and `load_threads`, read at startup. `network.max_open_files` now feeds the
  same per-worker budget instead of being ignored.
- **Parallel session restore (`rt-core`)**: torrent/sidecar/resume parsing uses
  `storage.load_threads` scoped threads, with results inserted in enumeration
  order so view and event ordering are unchanged.
- **Benchmark harness (`scripts/bench`)**: startup, lookup, multicall, recheck
  and disk-throughput measurements against a synthetic session.

### Changed

- **O(1) download lookup (`rt-core`)**: `DownloadList` and every `View` index
  downloads by info hash instead of scanning. Duplicate detection and lookup no
  longer grow with the torrent count; insertion order is preserved.
- **Hash passes flush once (`lt-storage`)**: one `sync_all` before the first
  piece read instead of one per piece. The engine already flushed before the
  pass and blocks new writes for its duration.
- **`d.multicall2` / `d.multicall.filtered` (`rt-core`)**: command columns are
  parsed once per call, not per (download x column). A 14k-torrent poll drops
  from ~47 s to ~0.2 s.

### Fixed

- **Nested command trust (`rt-rpc`)**: multicall wrappers resolved inner
  commands as trusted regardless of the connection, letting an untrusted SCGI
  client execute privileged commands. Handlers now inherit the caller's trust
  level.
- **Metadata identity (`lt-engine`)**: applying metadata with a different info
  hash is rejected instead of silently changing the download's identity.

## [0.13.0] - 2026-10-22

Payload-verification and rTorrent-parity release. Magnet links no longer
re-download data that already exists on disk, per-download variables match
rtorrent semantics, and the workspace passes clippy with warnings denied.

### Changed

- **Clippy gate (`CI`, workspace)**: `cargo clippy --workspace --all-targets -- -D
  warnings` now blocks merges. The workspace is clippy-clean without new lint
  suppressions; oversized internal APIs were regrouped into parameter structs
  and the fallible `*_nothrow` parsers now return a typed `ParseError`.
- **Version identity (`lt-common`)**: the client now presents `0.13.0` to peers,
  trackers and RPC clients. The peer ID prefix is `-RS0130-`, the tracker
  User-Agent is `rstorrent/0.13.0`, the DHT `v` is `RS0.13.0`, and the BEP 10
  handshake, `system.client_version` and gRPC `api_version` report the same
  version.

### Fixed

- **Per-download connection variables (`rt-core`)**: `d.connection_leech` reads
  and writes its own preference instead of aliasing `d.connection_seed`, so
  setting one no longer rewrites the other. Both setters validate against the
  shared connection-type option table (`leech`, `seed`, `initial_seed`,
  `metadata`).
- **Numbered custom variables (`rt-core`)**: `d.custom1`..`d.custom5` store to
  independent `rtorrent:customN` slots, matching rtorrent. They are no longer
  visible through `d.custom`, `d.custom.keys` or `d.custom.items`, and writes to
  the generic `d.custom` map no longer shadow them.
- **Magnet links verify an existing payload (`lt-engine`, `rt-core`)**: once
  `ut_metadata` delivers the info dictionary, a payload that is already in the
  target directory is hashed asynchronously (`HASHING`) instead of being
  downloaded again from zero. A complete file starts seeding, a partial file
  keeps its verified pieces and resumes only the rest. Peer requests and
  in-flight blocks are held back while the pass owns the data. Fresh magnet
  links, `.torrent` files and restored sessions keep their previous behaviour.

## [0.12.0] - 2026-10-22

Resume-durability and progress-accuracy release. Progress survives restarts and
crashes, verification is visible instead of blocking, and the client now reports
its real version to trackers, DHT nodes and RPC clients.

### Added

- **Periodic resume checkpoints (`rstorrent`, `rt-core`)**: a `session_checkpoint`
  schedule writes `<hash>.rtorrent` and the resume file for active downloads
  every 300 seconds (`session.checkpoint`), in addition to the existing
  1200-second full `session.save`. Stopping or pausing a download and finishing
  a hash pass also checkpoint immediately, and the forced double-`^Q` exit
  performs a bounded best-effort checkpoint before terminating.
- **Visible, non-blocking verification (`rt-core`, `rt-tui`)**: hashing an
  existing payload now runs on a worker thread with the engine lock released.
  The download reports `Checking`, the list shows `HASHING n/m`, and the
  transfer starts only after the pass completes.
- **Durable resume (`lt-storage`, `lt-engine`, `rt-core`)**: pieces marked
  complete from a disk write that has not been fsynced are saved as an optional
  `uncertain_pieces` resume extra. On restore those pieces are removed from the
  persisted bitfield and re-hashed instead of being trusted, so an OS crash
  cannot resurrect page-cache-only data. `flush_disk` clears the list after a
  successful fsync, and the key is omitted when every piece is durable.

### Changed

- **Progress percentages use verified bytes (`rt-tui`)**: list and details
  percentages, ETA and the "x / y MB" indicator derive from completed pieces
  instead of the session transfer counter. The transfer total remains visible
  as `Downloaded`.
- **Startup order (`rstorrent`)**: session entries are restored before CLI
  positional torrents are loaded, so loading a torrent that already has saved
  progress no longer commits an empty resume over it.
- **Version identity (`lt-common`)**: peer ID, tracker User-Agent, DHT `v`,
  BEP 10 extended-handshake `v`, `system.client_version` and the gRPC
  `api_version` are derived from the crate version instead of hard-coded
  strings. The peer ID prefix is now `-RS0120-` (was `-RS0100-`), trackers see
  `rstorrent/0.12.0`, and DHT nodes receive `RS0.12.0`.

### Fixed

- **Resume showed 0% after restart (`rt-core`)**: `total_downloaded`,
  `total_uploaded`, `state_changed` and `state_counter` are read back from the
  `.rtorrent` sidecar.
- **Statistics reset during hashing (`rt-core`)**: `downloaded_bytes` and
  `uploaded_bytes` no longer report zero while a hash pass runs.
- **Recheck skipped when a directory was already set (`rt-core`, `rstorrent`)**:
  existing payload is queued for verification regardless of whether the
  directory came from a default, a sidecar, or `d.directory.set`.

## [0.11.0] - 2026-10-21

Full rTorrent keyboard-shortcut parity release. The main download list, the
download details view and every secondary view now honour the legacy bindings,
including per-torrent limit tuning, throttle layouts, prompt control keys and
style-aware navigation. The release also fixes an `f.multicall` self-deadlock
and extends multicall coverage to single-file torrents.

### Added

- **Main download list shortcuts (`rt-tui`, `rstorrent`)**:
  - `^S` starts the focused download, `^D` (or `Delete` / vi `x`) stops an
    active download and erases a stopped one, `^K` sets `ignore_commands`,
    stops and closes, `^R` forces a complete re-hash and `^E` queues file
    creation/resizing through `f.multicall`.
  - `+`/`-` cycle the torrent priority, `^T` cycles the assigned throttle
    group, `I` toggles `d.ignore_commands`, `^B` enables initial seeding
    (BEP 16) and `U` deletes the tied file.
  - `^O` opens a `change_directory` prompt pre-filled with the current
    directory; the submitted path is applied with `d.directory.set`. It is
    rejected while the download is active, matching rTorrent.
- **Download details shortcuts (`rt-tui`)**:
  - `1`/`2` (`uploads_min`), `3`/`4` (`uploads_max`), `!`/`@`
    (`downloads_min`), `#`/`$` (`downloads_max`), `5`/`6` (`peers_min`) and
    `7`/`8` (`peers_max`) adjust per-torrent limits with saturation at zero.
  - `+`/`-` adjust priority, `t` requests a tracker announce and `T` a forced
    announce (only when an active tracker exists).
  - Sub-view keys now match rTorrent: `i` chunks seen, `u` transfer list,
    `p` peer list, `o` tracker list and `v` file list.
  - Tabs are switched with `Tab` / `Shift-Tab`; the numeric keys are reserved
    for limit tuning instead of switching tabs.
- **Throttle layouts (`rt-tui`, `rstorrent`)**:
  - Added `qwerty`, `azerty`, `qwertz` and `dvorak` throttle key maps,
    selectable through the new `keys.layout` / `keys.layout.set` RPC commands.
  - Throttle adjustments now use the configured
    `ui.throttle.global.step.{small,medium,large}` values; defaults are
    1 / 5 / 50 KB as in rTorrent.
  - Added `throttle.names` to list named throttle groups for `^T`.
- **Prompt and navigation (`rt-tui`)**:
  - `^G` aborts the active prompt, `^L` redraws it and `^Q` closes the prompt
    and requests shutdown.
  - A second `^Q` performs an immediate forced exit, bypassing the graceful
    network/session shutdown.
  - `Home` / `End` jump to the first and last download.
  - `^F` / vi `l` open the download details, `^B` / vi `h` return to the
    download list; `Space` leaves the log view.
  - The file list opens an information popup when Right is pressed on a file
    row (directories still expand in place).

### Changed

- `rt-tui` throttle defaults are now 1 / 5 / 50 KB instead of hard-coded
  5 / 50 / 500 KB steps.
- In the download details view, `t`/`T` no longer toggle views; they announce
  to the tracker. Transfer and chunk views are reached with `u` and `i`.
- Added `BackTab` translation for the terminal event reader.

### Fixed

- **`f.multicall` deadlock (`rt-core`)**: the command held a read lock on the
  download while invoking file setters that require a write lock, which hung
  `^E` and any `f.multicall` containing setters. The guard is now released
  before dispatch.
- **Single-file torrents in `f.multicall` (`rt-core`)**: multicall now uses the
  engine file list instead of `metadata.files`, so single-file downloads are
  included.

## [0.10.6] - 2026-10-20

Magnet session persistence release: downloads added from a magnet link now save
their fetched metadata as a static `.torrent` and are restored after a restart.
Placeholders whose metadata has not arrived yet survive restarts through the
original magnet URI, and session saves are triggered immediately when metadata
becomes available.

### Fixed

- **Magnet Session Persistence (`rt-core`, `lt-engine`, `rstorrent`)**:
  - Rebuild and persist the top-level torrent bencode (`info` + trackers) when
    metadata fetched through `ut_metadata` becomes available. Previously the
    wrapper kept `bencode == None`, so `Manager::build_commit_entry()` wrote only
    `.rtorrent` and `.libtorrent_resume` streams and never
    `<info-hash>.torrent`. `SessionManager::get_valid_entries()` then found no
    restorable entry and the torrent disappeared after every restart.
  - Added `lt_engine::network::download_info_bytes()` as the only safe source of
    verified raw `info` bytes. Re-encoding `TorrentMetadata` directly can change
    the info-hash when extension keys are present.
  - Queue a hash recheck for already existing payload files as soon as magnet
    metadata is applied, so an ISO/torrent payload downloaded earlier is
    verified and seeded.
  - Trigger an immediate session commit (`event.download.metadata`) instead of
    waiting for the 20-minute periodic save. Shutdown and `session.save`
    refresh pending magnet metadata before serializing as well.
  - Persist `magnet_uri` in the `.rtorrent` sidecar and recreate the placeholder
    on startup when `<info-hash>.torrent` is still missing. A magnet added and
    closed before metadata arrives is no longer lost.
  - Session restore now enumerates sidecar-only entries, transparently falls
    back to the persisted magnet URI, and silently skips entries already loaded
    from the command line instead of logging a duplicate warning.

- **Session Directory Lock (`rstorrent`)**:
  - Failure to acquire the session lock is now fatal at startup instead of
    logging an error and continuing without persistence, which could silently
    discard all torrent state when a second instance was started.

## [0.10.5] - 2026-10-20

Stability and state persistence release fixing session data loss on application restart, resolving existing file hash rechecking and seeding transitions, and ensuring continuous tracker announces and peer serving for completed downloads.

### Fixed

- **Session Persistence Out-of-the-Box (`rstorrent`, `rt-core`)**:
  - Automatically initialize default session directory (`$XDG_DATA_HOME/rstorrent/session` or `~/.local/share/rstorrent/session`) if no session path was configured via CLI or `.rtorrent.rc`.
  - Ensure session directory is created on disk (`fs::create_dir_all`) and expanded (`~`) before attempting to load or save session data.
  - Relaxed session filename validation to accept case-insensitive hex filenames.
  - Eliminated deadlock in `Control::try_load_torrent` by releasing write lock on `download_list` prior to invoking `save_session`.
  - Added integration test `test_session_persistence_default.rs` ensuring state is saved and restored across clean restarts.

- **Existing File Detection, Hash Recheck & Seeding (`lt-engine`, `rt-core`, `rstorrent`)**:
  - Added `queue_recheck_if_files_exist()` and `has_existing_files()` to automatically schedule chunk verification when adding or restoring torrents that have existing files in their destination directory.
  - Fixed download directory assignment in `try_load_torrent` and `apply_load_command` (`directory.default`) so file paths are properly located before rechecking pieces.
  - Fixed premature coordinator termination in `NetworkRuntime` by running `DownloadNetworkCoordinator` whenever a torrent is active (even when `is_done == true`).
  - Fixed tracker announce suppression in `lt-engine::network` to announce completed torrents with `left: 0` and keep listening for peer requests.
  - Added integration test `test_existing_file_seeding.rs` verifying complete/partial existing file rechecking, completion transition, and seeding state.


## [0.10.4] - 2026-10-20

Maintenance release unifying application naming to `rstorrent` across the TUI window title and runtime messages.

### Changed

- **Unified Application Branding (`rt-tui`, `rstorrent`)**:
  - Updated main window view title in `RootUI::switch_view` to display `rstorrent - View: <name>` (previously `rTorrent - View: <name>`).
  - Standardized panic handler reporting to prefix messages with `rstorrent: panic`.
  - Updated crate documentation headers, gRPC crate description, and test expectations to consistently reference `rstorrent`.

## [0.10.3] - 2026-10-20

Performance and stability release introducing 1-second background refresh throttling across all TUI views and detail tabs to eliminate lock contention, fixing a re-entrant deadlock in the Info detail tab, and adding verification tests.

### Added

- **TUI Refresh Throttling & Lock Contention Optimization (`rt-tui`)**:
  - Implemented 1-second background refresh throttling across all tabs in `WindowDownloadDetails` (`Info`, `Peers`, `Files`, `Trackers`, `Chunks`, `Transfers`).
  - Implemented 1-second background refresh throttling across dedicated inspection windows: `PeerListWindow` (`'4'`), `ChunksSeenWindow` (`'5'`), `TransferListWindow` (`'6'`), `FileListWindow` (`'v'`), and `TrackerListWindow` (`'t'`).
  - Preserved instantaneous responsiveness: navigation, paging, and scrolling draw from memory caches without locking the engine; tab switching and torrent switching immediately invalidate caches for zero latency.
  - Eliminated high-frequency lock contention on `Download` (`RwLock` and `EngineDownload` mutex) and visual jitter during high-throughput downloads.
  - Added throttling and cache invalidation unit tests in `test_ui_details.rs` and `test_ui_chunks_transfer.rs`.

### Fixed

- **Re-entrant Deadlock in Torrent Details Info Tab**:
  - Fixed deadlock in `WindowDownloadDetails::render_info_tab` where `dl.download()` was held while calling `dl.downloaded_bytes()`. Query now directly inspects engine counters and releases guards safely.

## [0.10.2] - 2026-10-20

Point release overhauling the TUI torrent detail views, adding full per-peer download/upload rate and progress tracking, introducing auto-refresh for file completion, and fixing UI key navigation.

### Added

- **Torrent Details View Overhaul (`rt-tui`)**:
  - Implemented dynamic data rendering across all tabs in `WindowDownloadDetails`:
    - `[1: Info]`: Live transfer rates (Up/Down KB/s), downloaded/uploaded totals, ratio, ETA, connected peers count, and active tracker summary.
    - `[2: Peers]`: Live peer list populated from `connection_list()`, displaying endpoint (`IP:Port`), client identification, protocol flags (`EINPSBO`), download and upload speeds, downloaded/uploaded totals, and piece completion percentage.
    - `[3: Files]`: Live file status reporting actual file priority (`off`, `normal`, `high`), file size, completed bytes, and progress percentage.
    - `[4: Trackers]`: Real-time tracker status from `TrackerController` (`Active`, `Connect`, `Failed`, `Disabled`, `Idle`), protocol type (`HTTP`, `UDP`, `DHT`), scrape statistics (seeders/leechers), and announce countdown.
    - `[5: Chunks]`: Chunk distribution overview, piece size, integrity progress bar, and active download count.
    - `[6: Transfers]`: Active block transfer monitoring from the engine delegator.
  - Added sorting in the peer tab: actively downloading peers (highest download rate) and peers with the most downloaded bytes automatically sort to the top of the list.
  - Added responsive layout in the peer tab: compact format designed to fit within standard 80-column terminals without clipping download statistics, expanding to include upload and piece completion on wider displays (>= 105 cols).
  - Added `PageUp` and `PageDown` scrolling support across detail tabs.
  - Added dedicated unit test suite for download details (`test_ui_details.rs`).

- **Per-Peer Transfer Accounting (`lt-engine`)**:
  - Wired peer-level download accounting (`peer.add_downloaded`) upon receiving `PeerMessage::Piece` blocks in `network.rs`, enabling real-time per-peer download rates and downloaded byte totals.
  - Wired peer-level upload accounting (`dl.record_upload_for_peer`) when sending block payloads.

- **Auto-Refresh for File View (`rt-tui`)**:
  - Added `auto_refresh` support to `WindowFileList` (`'v'` view) to dynamically reflect chunk completion and progress in real time without requiring manual refresh (`'r'`).

### Fixed

- **Torrent Details Tab 2 Stub**:
  - Fixed hardcoded `[No peers currently connected]` stub in `WindowDownloadDetails` that prevented connected peers from ever being displayed after pressing `Enter` and navigating to tab 2.
- **Lock Deadlock in Detail Files Tab**:
  - Fixed reentrant mutex deadlock in `WindowDownloadDetails` when querying file progress while holding the engine download guard.
- **TUI Key Routing in Detail Views**:
  - Guarded keys `7` through `0` while in `ActiveScreen::DownloadDetails` to prevent unintentional switching of background download list views.

## [0.10.1] - 2026-10-20

Point release resolving interactive terminal paste issues, fixing platform-specific test failures on macOS, enabling blocking multi-platform CI, and enforcing codebase-wide formatting standards.

### Added

- **Bracketed Paste Support in TUI**:
  - Implemented terminal bracketed paste event handling (`TerminalEvent::Paste`) in the main event loop.
  - Added `insert_str` across `InputTarget`, `TextInput`, `PathInput`, and `InputManager`.
  - Added automatic prompt activation: pasting a torrent URL or magnet link from the main download view now automatically initializes the `load.start` prompt with the pasted payload.
  - Automatic newline and carriage return sanitization (`\r\n`) to prevent corruption of single-line input prompts.

### Fixed

- **macOS (Darwin) Portability & Test Suite**:
  - Replaced hardcoded `/bin/false` and `/bin/true` process invocations in `rt-core` tests with portable PATH/prefix resolution (`bin_prog`), resolving test panics on macOS where those utilities reside in `/usr/bin`.
  - Fixed temporary directory path assertions in `test_command_session` by canonicalizing `/var` symlinks (resolving `/private/var` mismatch on Darwin).
  - Promoted macOS CI test runner (`macos-latest`) to fully gating/blocking (removed `continue-on-error`).
  - Enhanced `scripts/package.sh` with automatic ad-hoc code signing (`codesign -s -`) for macOS release artifacts to prevent Gatekeeper execution failures on Apple Silicon.

### Changed

- **Code Formatting & CI Enforcement**:
  - Executed a unified `cargo fmt --all` pass across the entire workspace (231 files).
  - Made the `rustfmt` check in GitHub Actions CI strictly required and blocking.

## [0.10.0] - 2026-10-19

First public release of the clean `rstorrent` repository: a pure-Rust,
memory-safe BitTorrent client and engine that re-implements the rTorrent and
libtorrent architecture, including native gRPC support for Flood.

### Added

- **Pure-Rust BitTorrent engine** (`lt-engine`) with chunk selection, disk
  I/O, capacity management, resume data and a Tokio-based network runtime.
- **Protocol support**: BEP 3 (core), BEP 6 (fast extension), BEP 9
  (`ut_metadata` / magnet links), BEP 10 (extension protocol), BEP 11 (PEX),
  BEP 15 (UDP trackers with scrape), BEP 16 (super-seeding), Mainline DHT,
  MSE/PE encryption (RC4 + Diffie-Hellman).
- **rTorrent-compatible RPC surface**:
  - SCGI server over TCP and Unix domain sockets.
  - XML-RPC and JSON-RPC dialects with `d.multicall2`, `f/t/p.multicall`,
    command redirects and event handlers.
  - Drop-in compatibility with Flood and ruTorrent.
- **Native gRPC API** (`crates/rt-grpc`, package `rtorrent.v1`):
  - `TorrentService`: `ListTorrents`, `GetTorrent`, `AddTorrent`,
    `ControlTorrent`, `RemoveTorrent`, `SetTorrentPriority`, server-streaming
    `StreamEvents`.
  - `SystemService`: `GetVersion`, `GetSessionStats`, server-streaming
    `StreamStats`, `Shutdown`.
  - `CommandService`: `Execute`, `Multicall`, `ListCommands` (full command-map
    bridge with XML-RPC-compatible target routing).
  - TCP (`--grpc-port`) and Unix socket (`--grpc-socket`) transports, 64 MiB
    message limit.
- **Magnet link support** in `load.start` / `load.normal` with metadata
  retrieval over `ut_metadata` and automatic metadata synchronization to the
  command layer.
- **Sequential download mode** (`d.down.sequential` / `.set`) backed by a
  sequential chunk-selection strategy.
- **Commands and compatibility shims**: `cat`, `true`, `false`,
  `d.timestamp.last_active`, `d.creation_date`, `d.save_full_session`,
  `d.down.sequential`.
- **Interactive TUI** (Ratatui/Crossterm) and headless daemon mode.
- **Dual binaries**: `rstorrent` and an `rtorrent` compatibility symlink.
- **Docker images and Compose stacks** for daemon and Flood integration.
- **CI and release automation**: formatting, clippy, workspace tests, release
  builds for Linux (x86_64/aarch64) and macOS (x86_64/arm64), `.deb` and
  tarball packaging, checksums and GitHub Releases.

### Fixed

- CLI overrides (`-d/--directory`, `-s/--session`, `-p/--port`) are applied as
  trusted commands; previously `directory.default` stayed at `./`, which made
  Flood default to a destination outside its allowed paths.
- Erased downloads are removed from all views, so `d.multicall2` no longer
  returns phantom rows with failing getters.
- `system.multicall` batches are served through `CommandService.Multicall`
  (the DSL command itself is not part of the rTorrent command map).
- Capability detection keeps expression-based method calls (`cat="..."`)
  instead of dropping them, restoring the tracker column in Flood.
- `Load` commands accept magnet URIs instead of treating them as file paths.

### Known limitations

- **Windows is not supported**: the daemon relies on Unix domain sockets,
  signals and POSIX file APIs.
- The typed `GetTorrent` response does not yet expose live peer lists, per-file
  completion/priority or tracker status; clients should use `CommandService`
  (as Flood does).
- `d.directory_base` follows rstorrent's storage layout, where multi-file paths
  include the base directory; clients that add multi-file torrents with an
  explicit base path must pass the parent directory.
- macOS builds and packages are released, but a few storage tests still assert
  Linux-only `posix_fadvise` counters; macOS test results are advisory in CI.

[Unreleased]: https://github.com/chmuri/rstorrent/compare/v0.16.1...HEAD
[0.16.1]: https://github.com/chmuri/rstorrent/releases/tag/v0.16.1
[0.16.0]: https://github.com/chmuri/rstorrent/releases/tag/v0.16.0
[0.15.0]: https://github.com/chmuri/rstorrent/releases/tag/v0.15.0
[0.14.0]: https://github.com/chmuri/rstorrent/releases/tag/v0.14.0
[0.13.0]: https://github.com/chmuri/rstorrent/releases/tag/v0.13.0
[0.12.0]: https://github.com/chmuri/rstorrent/releases/tag/v0.12.0
[0.11.0]: https://github.com/chmuri/rstorrent/releases/tag/v0.11.0
[0.10.4]: https://github.com/chmuri/rstorrent/releases/tag/v0.10.4
[0.10.3]: https://github.com/chmuri/rstorrent/releases/tag/v0.10.3
[0.10.2]: https://github.com/chmuri/rstorrent/releases/tag/v0.10.2
[0.10.1]: https://github.com/chmuri/rstorrent/releases/tag/v0.10.1
[0.10.0]: https://github.com/chmuri/rstorrent/releases/tag/v0.10.0
