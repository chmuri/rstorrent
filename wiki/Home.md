# rtorrent Wiki

Welcome to the **rstorrent** wiki!

`rstorrent` is a modern, high-performance, and memory-safe BitTorrent client and engine written from scratch in pure Rust. It provides full compatibility with the classic rTorrent ecosystem while offering modern features such as uTP (BEP 29), UPnP IGD port forwarding, a modern TOML configuration system, a lock-free asynchronous I/O engine, and a next-generation terminal user interface.

---

## Wiki Pages

- **[Configuration Guide](Configuration-Guide.md)**: Details on configuring rtorrent using modern `rstorrent.toml` or classic `.rtorrent.rc`.
- **[Terminal UI and Keybindings](Terminal-UI-and-Keybindings.md)**: Full guide to mouse navigation, keyboard shortcuts, category tabs, and the torrent inspector drawer.
- **[Protocol Support & Performance](Protocol-Support-and-Performance.md)**: BitTorrent Enhancement Proposals (BEP) matrix, uTP/LEDBAT congestion control, and scalability benchmarks.
- **[gRPC and Remote Control](gRPC-and-Remote-Control.md)**: Running headless daemon mode and connecting via `rstorrent-tui` or custom gRPC integrations.

---

## Quick Start

### Installation

Download pre-built binaries or `.deb` packages from the [GitHub Releases](https://github.com/chmuri/rstorrent/releases) page:

```bash
# Ubuntu / Debian
sudo apt install ./rstorrent_0.20.1_amd64.deb

# Or run standalone
./rstorrent
```

### Running with TOML configuration

Create `rstorrent.toml` in your home configuration directory (`~/.config/rstorrent/rstorrent.toml`):

```toml
[network]
port = 50000
download_throttle_kb = 0
upload_throttle_kb = 2000

[storage]
directory = "~/Downloads"
session_directory = "~/.local/share/rstorrent/session"
watch_directory = "~/Downloads/watch"

[dht]
mode = "auto"
port = 6881
```

Then start:

```bash
rstorrent
```
