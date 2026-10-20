# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/chmuri/rstorrent/compare/v0.10.1...HEAD
[0.10.1]: https://github.com/chmuri/rstorrent/releases/tag/v0.10.1
[0.10.0]: https://github.com/chmuri/rstorrent/releases/tag/v0.10.0
