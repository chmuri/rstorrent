# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/chmuri/rstorrent/compare/v0.13.0...HEAD
[0.13.0]: https://github.com/chmuri/rstorrent/releases/tag/v0.13.0
[0.12.0]: https://github.com/chmuri/rstorrent/releases/tag/v0.12.0
[0.11.0]: https://github.com/chmuri/rstorrent/releases/tag/v0.11.0
[0.10.4]: https://github.com/chmuri/rstorrent/releases/tag/v0.10.4
[0.10.3]: https://github.com/chmuri/rstorrent/releases/tag/v0.10.3
[0.10.2]: https://github.com/chmuri/rstorrent/releases/tag/v0.10.2
[0.10.1]: https://github.com/chmuri/rstorrent/releases/tag/v0.10.1
[0.10.0]: https://github.com/chmuri/rstorrent/releases/tag/v0.10.0
