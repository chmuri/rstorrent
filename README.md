# rstorrent

[![CI](https://github.com/chmuri/rstorrent/actions/workflows/ci.yml/badge.svg)](https://github.com/chmuri/rstorrent/actions/workflows/ci.yml)
[![License: GPL v2+](https://img.shields.io/badge/License-GPL%20v2%2B-blue.svg)](LICENSE)
[![Rust: 2021](https://img.shields.io/badge/Rust-2021-orange.svg)](Cargo.toml)

A modern, high-performance, memory-safe BitTorrent client and engine written in pure Rust. `rstorrent` is a complete re-implementation and architectural evolution of the legendary [rTorrent](https://github.com/rakshasa/rtorrent) and [libtorrent](https://github.com/rakshasa/libtorrent) C++ codebase.

It provides 100% behavioral parity with rTorrent's RPC and SCGI protocol, allowing drop-in compatibility with frontends such as [Flood](https://github.com/jesec/flood) and [ruTorrent](https://github.com/Novik/ruTorrent), while offering memory safety, non-blocking asynchronous I/O, and modern concurrency.

---

## Key Features

- **Pure Rust & Memory Safe**: Zero segmentation faults, robust error handling, and structured concurrency built on [Tokio](https://tokio.rs).
- **Full BitTorrent Protocol Support**:
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
  - Standard SCGI server over TCP (`0.0.0.0:5000`) and UNIX Domain Sockets (`/session/rtorrent.sock`).
  - XML-RPC and JSON-RPC dialects, supporting `d.multicall2`, commands, filtering, and events.
  - Works out-of-the-box with **Flood**, **ruTorrent**, scripts, and automation tools.
- **Interactive TUI & Daemon Mode**:
  - Rich Terminal User Interface powered by [Ratatui](https://github.com/ratatui/ratatui) and [Crossterm](https://github.com/crossterm-rs/crossterm).
  - Headless daemon mode (`--daemon`) for servers and Docker containers.
- **Dual Binary Compatibility**:
  - Provides both `rstorrent` and `rtorrent` binaries out of the box for backward compatibility with existing scripts.

---

## Architecture & Modular Workspace

`rstorrent` is organized as a clean, highly modular workspace consisting of 14 specialized crates:

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
│   ├── rt-core       # Session manager, download views, schedules, throttle groups
│   ├── rt-tui        # Terminal UI (Ratatui / Crossterm), views, canvas buffer, frame balancing
│   └── rstorrent     # CLI entry point, signal handlers, lifecycle supervisor
```

---

## Quick Start

### 1. Using Docker Compose (Recommended)

The easiest way to run `rstorrent` along with the modern **Flood Web UI**:

```bash
# Clone the repository
git clone https://github.com/chmuri/rstorrent.git
cd rstorrent

# Build and start services
docker compose up -d
```

- **Flood Web UI**: Open [http://localhost:3000](http://localhost:3000) in your browser.
- **SCGI Port**: `127.0.0.1:5000` (TCP) and `./data/session/rtorrent.sock` (UNIX socket).
- **BitTorrent P2P Port**: `50000` (TCP).
- **DHT Port**: `6881` (UDP).

### 2. Building from Source

#### Prerequisites
- Rust 1.75+ (stable toolchain)
- Cargo

```bash
# Build release binaries (rstorrent and rtorrent)
cargo build --release -p rstorrent

# Run in daemon mode
./target/release/rstorrent --daemon

# Or run interactive TUI
./target/release/rstorrent
```

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
```

---

## Testing & Quality Assurance

`rstorrent` has an exhaustive test suite covering unit tests, integration tests, fuzz targets, and end-to-end scenarios across all 14 crates:

```bash
# Run all tests across the workspace
cargo test --workspace

# Run static checks
cargo clippy --workspace --all-targets
```

---

## Contributing

Contributions are warmly welcomed! Please read [CONTRIBUTING.md](CONTRIBUTING.md) for details on our code style, test coverage requirements, and pull request workflow.

---

## License

This project is licensed under the **GNU General Public License v2.0 or later** ([GPL-2.0-or-later](LICENSE)), preserving full license compatibility with the original rTorrent and libtorrent projects.
