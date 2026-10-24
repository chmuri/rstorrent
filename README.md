# rstorrent

[![CI](https://github.com/chmuri/rstorrent/actions/workflows/ci.yml/badge.svg)](https://github.com/chmuri/rstorrent/actions/workflows/ci.yml)
[![Docker](https://github.com/chmuri/rstorrent/actions/workflows/docker.yml/badge.svg)](https://github.com/chmuri/rstorrent/pkgs/container/rstorrent)
[![Release](https://github.com/chmuri/rstorrent/actions/workflows/release.yml/badge.svg)](https://github.com/chmuri/rstorrent/releases)
[![License: GPL v2+](https://img.shields.io/badge/License-GPL%20v2%2B-blue.svg)](LICENSE)
[![Rust: 2021](https://img.shields.io/badge/Rust-2021-orange.svg)](Cargo.toml)
[![Platforms](https://img.shields.io/badge/platforms-Linux%20%7C%20macOS-informational.svg)](#supported-platforms)

## About

`rstorrent` is a modern, high-performance, and memory-safe BitTorrent client and engine written from scratch in pure Rust. It is designed as an rTorrent-compatible alternative with a modular workspace, a Tokio-based engine, and a terminal-first runtime.

The project contains active implementations for core BitTorrent functionality, an SCGI server, XML-RPC/JSON-RPC command handling, and a native gRPC service layer. However, this is still an evolving codebase: compatibility with the full rTorrent command surface, frontend integrations, and every ecosystem feature is not yet complete and should be treated as work in progress rather than full parity.

---

## Key Features

- **Pure Rust & Memory Safe**: Zero segmentation faults, robust error handling, and structured concurrency built on [Tokio](https://tokio.rs).
- **BitTorrent Protocol Support**:
  - **BEP 3**: Core BitTorrent Protocol.
  - **BEP 6**: Fast Extension (`Allowed Fast`, `Suggest`, `Have All`, `Have None`, `Reject Request`).
  - **BEP 9**: Extension for Peers to Send Metadata Files (`ut_metadata`, magnet links).
  - **BEP 10**: Extension Protocol.
  - **BEP 11**: Peer Exchange (PEX).
  - **BEP 15**: UDP Tracker Protocol with scrape support.
  - **BEP 16**: Super-Seeding (Initial Seeding).
  - **MSE / PE**: Message Stream Encryption / Protocol Encryption (RC4 + Diffie-Hellman 768-bit).
  - **Mainline DHT**: Kademlia Distributed Hash Table with bootstrap and caching.
- **SCGI & RPC Compatibility**:
  - SCGI listener and parser code are present in the workspace (`rt-scgi`, `rt-rpc`).
  - XML-RPC and JSON-RPC processing exists for command routing and compatibility layers.
  - Intended compatibility with tooling such as **Flood**, **ruTorrent**, and custom scripts is real, but command coverage and full ecosystem parity are still being expanded.
- **Interactive TUI & Daemon Mode**:
  - Rich Terminal User Interface powered by [Ratatui](https://github.com/ratatui/ratatui) and [Crossterm](https://github.com/crossterm-rs/crossterm).
  - Headless daemon mode (`--daemon`) for servers and Docker containers.
- **Native gRPC API**:
  - Tonic-based `rtorrent.v1` services exist in [`crates/rt-grpc`](crates/rt-grpc).
  - This layer is an important compatibility and integration surface, but it is still best treated as an actively evolving API rather than a fully complete feature-equivalent replacement for the legacy rTorrent toolchain.
- **Dual Binary Compatibility**:
  - Provides both `rstorrent` and `rtorrent` binaries out of the box for backward compatibility with existing scripts.
- **Scales to Large Sessions**:
  - O(1) hash-indexed lookups, parallel session restore, a bounded recheck pool and sharded disk workers keep 14,000+ torrents responsive; see [Performance and scaling](#performance-and-scaling).

---

## Architecture & Modular Workspace

`rstorrent` is organized as a clean, highly modular workspace consisting of 15 specialized crates:

```
rstorrent/
├── crates/
│   ├── lt-common     # Fundamental types, Bencode parser/serializer, Rate, Bitfield, HashString
│   ├── lt-sys        # System primitives: lockfiles, scheduler, log buffers, safe mmap, fadvise
│   ├── lt-crypto     # SHA-1, RC4 with discard 1024, Diffie-Hellman 768-bit (MSE/PE)
│   ├── lt-net        # Socket abstraction, IPv4/IPv6, bandwidth choking & rate limits
│   ├── lt-storage    # Chunk storage, piece selection (rarest-first, sequential), fast-resume (.libtorrent_resume)
│   ├── lt-dht        # Mainline DHT (Kademlia routing table, bootstrap, tokens, caching)
│   ├── lt-tracker    # HTTP/HTTPS & UDP tracker client, multi-tier fallback, backoff
│   ├── lt-protocol   # Peer wire protocol, MSE state machine, BEP 6, BEP 9, BEP 10, BEP 11, BEP 16
│   ├── lt-engine     # Engine coordinator, disk threads, download lifecycle state machine
│   ├── rt-rpc        # Command DSL engine, command registry, XML-RPC & JSON-RPC parser, Lua bridge
│   ├── rt-scgi       # Non-blocking SCGI server over TCP & UNIX sockets with keep-alive
│   ├── rt-grpc       # gRPC rtorrent.v1 service endpoints & push streams (Tonic)
│   ├── rt-core       # Session manager, download views, schedules, throttle groups
│   ├── rt-tui        # Terminal UI (Ratatui / Crossterm), views, canvas buffer, frame balancing
│   └── rstorrent     # CLI entry point, signal handlers, lifecycle supervisor
```

---

## Current compatibility status

This project should be considered a work in progress rather than a fully complete, drop-in replacement for every historical rTorrent deployment.

The repository already contains substantial implementations for:

- core BitTorrent mechanics,
- SCGI handling,
- XML-RPC / JSON-RPC parsing and dispatch,
- gRPC endpoints,
- the terminal UI,
- and a modular engine/runtime architecture.

However, feature completeness varies by subsystem. Older rTorrent commands, frontends, and edge-case protocol behavior may still be incomplete or require additional compatibility work. The goal is compatibility with the rTorrent ecosystem, but it is not yet safe to assume full protocol or command parity across all known integrations.

---

## Performance and scaling

`rstorrent` is designed to stay responsive with large sessions, not just a
handful of torrents. On an 8-core host with a local ext4 volume, a synthetic
session of **14,000 paused torrents** restores in about **1 second** and keeps
lookups flat:

| Metric | 14,000 torrents |
|---|---:|
| Startup to fully restored | ~0.9–1.2 s |
| RSS | ~145 MB (~10 KB/torrent) |
| `d.hash` p99 | ~0.2 ms |
| `d.multicall2` (12 columns) p50 | ~0.2 s |
| Recheck trigger (`d.check_hash`) | ~3 ms, bounded queue |

How it scales:

- **O(1) lookup** — downloads are indexed by info hash in `DownloadList` and in
  every view, while insertion order (and therefore view ordering) is preserved.
- **Parallel restore** — session entries are parsed on a bounded thread pool,
  then inserted in their original order so events and views stay deterministic.
- **Bounded rechecks** — hash passes run on a fixed worker pool with a FIFO
  queue instead of one OS thread per torrent. Triggering a recheck no longer
  blocks the RPC connection.
- **Sharded disk I/O** — writes, reads and syncs for a torrent are routed to a
  dedicated disk worker chosen by info hash, preserving per-torrent FIFO order
  while letting unrelated torrents proceed in parallel.
- **One flush per hash pass** — the engine already owns the data during
  verification, so the pass flushes the disk worker once instead of per piece.

Storage settings are read at startup (restart to apply):

```ini
# Disk workers. Default 1; raise for concurrent I/O across torrents.
storage.disk_threads = 1

# Per-worker open-file cache.
storage.max_open_files = 64

# Concurrent hash passes. Defaults to min(4, CPU cores).
storage.hash_concurrency = 4

# Session-parse threads during startup. Defaults to min(4, CPU cores).
storage.load_threads = 4

# Files a hash pass keeps open (64..=1024).
storage.hash_read_cache = 64
```

Note that hash reads bypass the disk worker by design: recheck throughput
follows `storage.hash_concurrency`, while `storage.disk_threads` helps
concurrent writes, reads and syncs. FUSE/union mounts remain a single
bottleneck regardless of worker count.

See [docs/scalability.md](docs/scalability.md) for the thread model, descriptor
budget and invariants, and [`scripts/bench`](scripts/bench) for the harness used
to produce the numbers above.

---

## Quick Start

### 1. Using Docker Compose (Recommended)

The easiest way to run `rstorrent` using the pre-built image from GitHub Container Registry (`ghcr.io/chmuri/rstorrent:latest`) along with the modern **Flood Web UI**:

```bash
# Clone the repository
git clone https://github.com/chmuri/rstorrent.git
cd rstorrent

# Start services (pulls pre-built ghcr.io image automatically)
docker compose up -d
```

You can also pull the container image directly:
```bash
docker pull ghcr.io/chmuri/rstorrent:latest
```

- **Flood Web UI**: Open [http://localhost:3000](http://localhost:3000) in your browser.
- **SCGI Port**: `127.0.0.1:5000` (TCP) and `./data/session/rtorrent.sock` (UNIX socket).
- **BitTorrent P2P Port**: `50000` (TCP).
- **DHT Port**: `6881` (UDP).

### 2. Building from Source

#### Prerequisites
- Rust 1.99+ (stable toolchain)
- Cargo

```bash
# Build release binaries (rstorrent and rtorrent)
cargo build --release -p rstorrent

# Run in daemon mode
./target/release/rstorrent --daemon

# Or run interactive TUI
./target/release/rstorrent
```

### Running on a server over SSH

The interactive TUI is tied to the terminal that started it: closing the SSH
session sends `SIGHUP` and the process performs a clean shutdown. For
unattended downloads use daemon mode under `tmux`/`screen`, `nohup` or a
systemd unit:

```bash
tmux new -s rstorrent
rstorrent --daemon --session /var/lib/rstorrent/session "magnet:?xt=urn:btih:..."
```

The session directory stores both the static `.torrent` files and resume data,
so downloads are restored after a restart. Magnet downloads persist their
metadata as soon as it is fetched; while metadata is still missing the original
magnet URI is kept in the `.rtorrent` sidecar so the placeholder can be
recreated.

Progress is checkpointed while the daemon runs (every 300 seconds by default,
plus on stop/pause, on hash completion and on shutdown), so a crash or a forced
exit loses at most a few minutes of transfer. Payload already on disk is
verified in the background after a restart: the status shows `HASHING n/m`
while pieces are checked, and the progress column reports verified bytes rather
than the session transfer counter. Pieces written but not yet fsynced are
recorded in the resume file and re-verified after a crash instead of being
trusted.

---

## Releases & Packages

Every `v*` tag is built by the [Release workflow](.github/workflows/release.yml)
and published on the [Releases](https://github.com/chmuri/rstorrent/releases)
page together with SHA-256 checksums. See [CHANGELOG.md](CHANGELOG.md) for the
full change history.

### Supported platforms

| Platform | Packages |
| --- | --- |
| Linux x86_64 (glibc) | `.deb` (`amd64`), `.tar.gz` |
| Linux aarch64 (glibc) | `.deb` (`arm64`), `.tar.gz` |
| macOS x86_64 | `.tar.gz` |
| macOS arm64 (Apple Silicon) | `.tar.gz` |
| Windows | Not supported (Unix sockets, signals and POSIX APIs) |

### Debian / Ubuntu

```bash
# Download the .deb for your architecture from the Releases page, then:
sudo apt install ./rstorrent_<version>_amd64.deb
# Installs /usr/bin/rstorrent and the /usr/bin/rtorrent compatibility symlink.
```

### Tarball (any Linux distribution, macOS)

```bash
tar -xzf rstorrent-<version>-<target>.tar.gz
sudo install -m 0755 rstorrent-<version>/rstorrent /usr/local/bin/
```

Every release also ships `*-sha256` files; verify with `sha256sum -c`.

---

## Native gRPC API

Besides SCGI/XML-RPC, the daemon exposes a native gRPC server
([`crates/rt-grpc`](crates/rt-grpc), package `rtorrent.v1`) with the same
command map underneath:

```bash
# Unix domain socket (recommended, no TLS needed on a local socket)
rstorrent --daemon --grpc-socket /run/rstorrent/grpc.sock

# or TCP
rstorrent --daemon --grpc-port 50051
```

| Service | Methods |
| --- | --- |
| `TorrentService` | `ListTorrents`, `GetTorrent`, `AddTorrent`, `ControlTorrent`, `RemoveTorrent`, `SetTorrentPriority`, `StreamEvents` (server streaming) |
| `SystemService` | `GetVersion`, `GetSessionStats`, `StreamStats` (server streaming), `Shutdown` |
| `CommandService` | `Execute`, `Multicall`, `ListCommands` (full rTorrent DSL bridge) |

`CommandService` understands rTorrent's command syntax (`load.*`,
`d.multicall2`, `f/t/p.multicall`, `session.path`, `d.custom1`, …), including
XML-RPC-style target routing, so existing integrations can be ported one call
at a time. Flood's dedicated `rstorrent` connector consumes the gRPC streams;
see the Flood repository for a ready-to-run `docker-compose.rstorrent.yml`.

---

## Configuration

`rstorrent` automatically looks for configuration files in standard locations:
1. `$XDG_CONFIG_HOME/rstorrent/rstorrent.rc` (or `rtorrent/rtorrent.rc`)
2. `~/.config/rstorrent/rstorrent.rc` (or `~/.config/rtorrent/rtorrent.rc`)
3. `~/.rstorrent.rc` (or `~/.rtorrent.rc`)

Example `.rstorrent.rc`:

```ini
# Default download directory
directory.default.set = /downloads

# Session directory (stores resume state and torrent files)
session.path.set = /session

# Network listening port range
network.port_range.set = 50000-50000

# Distributed Hash Table (DHT)
dht.port.set = 6881

# SCGI server for Web UI (Flood / ruTorrent)
network.scgi.open_port = 0.0.0.0:5000
network.scgi.open_local = /session/rtorrent.sock

# Storage workers (read at startup)
storage.disk_threads = 1        # 4 on a dedicated NVMe setup
storage.max_open_files = 64     # per worker
storage.hash_concurrency = 4    # default min(4, CPU cores)
storage.load_threads = 4        # default min(4, CPU cores)
```

---

## Testing & Quality Assurance

`rstorrent` has an exhaustive test suite covering unit tests, integration tests, fuzz targets, and end-to-end scenarios across all 15 crates:

```bash
# Run all tests across the workspace
cargo test --workspace

# Run static checks
cargo clippy --workspace --all-targets -- -D warnings

# Reproduce the scalability numbers on a synthetic session
scripts/bench/run_all.sh --count 14000
```

---

## Contributing

Contributions are warmly welcomed! Please read [CONTRIBUTING.md](CONTRIBUTING.md) for details on our code style, test coverage requirements, and pull request workflow.

---

## Changelog & Security

- [CHANGELOG.md](CHANGELOG.md) — release history.
- [SECURITY.md](SECURITY.md) — how to report vulnerabilities.

---

## License

This project is licensed under the **GNU General Public License v2.0 or later** ([GPL-2.0-or-later](LICENSE)), preserving full license compatibility with the original rTorrent and libtorrent project while remaining independent and modernized for Rust.
