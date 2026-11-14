# rstorrent Wiki

Welcome to the **rstorrent** wiki!

`rstorrent` is a modern, high-performance, memory-safe BitTorrent client and engine written from scratch in pure Rust. It is a drop-in companion to the classic rTorrent ecosystem — same command surface over XML-RPC/SCGI and gRPC — while adding modern capabilities: uTP (BEP 29) with LEDBAT congestion control, BEP 52 v2/hybrid Merkle torrents, UPnP IGD / NAT-PMP port forwarding, DHT with BEP 51, a TOML configuration system, a lock-free asynchronous I/O engine with sharded disk workers, a native Web UI, and a next-generation terminal user interface.

Current release: **v0.21.0**. Licensed under GPL-2.0-or-later.

---

## Wiki Pages

- **[Configuration Guide](Configuration-Guide)** — the three config layers (`rstorrent.toml`, `.rtorrent.rc`, Lua), file lookup order, full TOML reference, command namespaces, schedules, and environment variables.
- **[CLI Reference](CLI-Reference)** — every `rstorrent` and `rstorrent-tui` flag, config precedence, signals, and environment variables.
- **[Terminal UI & Keybindings](Terminal-UI-and-Keybindings)** — the two shipped terminal surfaces, views, layout, inspector, themes, and mouse support.
- **[Keyboard Shortcuts](Keyboard-Shortcuts)** — complete, source-verified key tables for both surfaces.
- **[Protocol Support & Performance](Protocol-Support-and-Performance)** — the BEP implementation matrix, transports (MSE, UPnP, NAT-PMP, SOCKS5/HTTP), uTP/LEDBAT, and known limitations.
- **[Performance & Scaling](Performance-and-Scaling)** — disk-thread tuning, piece memory, and scalability benchmarks.
- **[Architecture & Algorithms](Architecture-and-Algorithms)** — piece selection, sharded disk pool, BEP 52 Merkle verification, throttling, and choke heuristics.
- **[gRPC & Remote Control](gRPC-and-Remote-Control)** — headless daemon mode, the three gRPC services, connection security, and client integrations.
- **[XML-RPC & SCGI](XML-RPC-and-SCGI)** — listeners, netstring framing, codecs, authentication, the command surface, and compatibility stubs.
- **[Web UI](WebUI)** — the opt-in native Web UI: REST + WebSocket API, authentication, and TLS.

---

## Quick Start

### Installation

Download pre-built binaries or `.deb` packages from the [GitHub Releases](https://github.com/chmuri/rstorrent/releases) page:

```bash
# Ubuntu / Debian
sudo apt install ./rstorrent_0.21.0_amd64.deb

# Or run standalone
./rstorrent
```

### Running with TOML configuration

Create `rstorrent.toml` in your configuration directory (`~/.config/rstorrent/rstorrent.toml`):

```toml
[network]
port = 50000
download_throttle_kb = 0        # unlimited
upload_throttle_kb = 2000

[storage]
directory = "~/Downloads"
session = "~/.local/share/rstorrent/session"
watch_directory = "~/Downloads/watch"

[dht]
mode = "auto"
port = 6881
```

Then start:

```bash
rstorrent
```

See the [Configuration Guide](Configuration-Guide) for the full reference and the [CLI Reference](CLI-Reference) for all flags.

### Headless daemon + Web UI

```toml
[rpc]
grpc_port = 50051
grpc_token = "change-me"

[webui]
addr = "127.0.0.1:8080"
token = "change-me"
```

```bash
rstorrent --daemon
```

Connect a terminal client with `rstorrent-tui --endpoint 127.0.0.1:50051`, or open the Web UI at `http://127.0.0.1:8080`. See [gRPC & Remote Control](gRPC-and-Remote-Control) and [Web UI](WebUI).
