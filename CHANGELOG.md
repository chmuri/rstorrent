# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/chmuri/rstorrent/compare/v0.10.4...HEAD
[0.10.4]: https://github.com/chmuri/rstorrent/releases/tag/v0.10.4
[0.10.3]: https://github.com/chmuri/rstorrent/releases/tag/v0.10.3
[0.10.2]: https://github.com/chmuri/rstorrent/releases/tag/v0.10.2
[0.10.1]: https://github.com/chmuri/rstorrent/releases/tag/v0.10.1
[0.10.0]: https://github.com/chmuri/rstorrent/releases/tag/v0.10.0
