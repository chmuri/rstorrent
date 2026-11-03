# Configuration Guide

`rstorrent` supports two configuration formats:
1. **Modern `rstorrent.toml`**: Clean, human-readable TOML with structured tables for network, storage, RPC, and DHT.
2. **Classic `.rtorrent.rc`**: Backward-compatible rTorrent command scripting.

---

## File Lookup Order

`rstorrent` automatically looks for configuration files in standard locations following the XDG base directory specification:

1. `./rstorrent.toml` (or `./.rstorrent.toml`) in the current working directory.
2. `$XDG_CONFIG_HOME/rstorrent/rstorrent.toml`.
3. `~/.config/rstorrent/rstorrent.toml`.
4. `~/.rstorrent.toml`.
5. `$XDG_CONFIG_HOME/rstorrent/rstorrent.rc` (or `rtorrent/rtorrent.rc`).
6. `~/.config/rstorrent/rstorrent.rc` (or `~/.config/rtorrent/rtorrent.rc`).
7. `~/.rstorrent.rc` (or `~/.rtorrent.rc`).

You can override the configuration file path with `-c`:
```bash
rstorrent -c /path/to/custom_config.toml
```
To run without loading any configuration file, use `-n`:
```bash
rstorrent -n
```

---

## TOML Configuration Reference (`rstorrent.toml`)

```toml
[network]
# Primary listening port for incoming TCP and uTP connections
port = 50000

# Bind address (e.g., "0.0.0.0" or a specific network interface IP)
bind_address = "0.0.0.0"

# Maximum upload slots across active torrents
max_uploads = 8

# Maximum simultaneous active downloads
max_downloads = 16

# Rate limits in KB/s (0 = uncapped)
download_throttle_kb = 0
upload_throttle_kb = 2500

[storage]
# Default download destination
directory = "~/Downloads"

# State and session directory for resume data
session_directory = "~/.local/share/rstorrent/session"

# Real-time watch directory (auto-loads and starts new .torrent files)
watch_directory = "~/Downloads/watch"

[rpc]
# gRPC server port for remote connections (rstorrent-tui, Flood, etc.)
grpc_port = 50051

# gRPC UNIX domain socket
grpc_socket = "/tmp/rstorrent-grpc.sock"

# Legacy SCGI ports
scgi_port = 5000
scgi_socket = "/tmp/rstorrent-scgi.sock"

[dht]
# DHT mode: "auto", "on", or "off"
mode = "auto"

# DHT UDP port
port = 6881
```

---

## Legacy `.rtorrent.rc` Reference

Classic commands remain fully functional:

```ini
directory.default.set = /downloads
session.path.set = /session
network.port_range.set = 50000-50000
dht.port.set = 6881
network.scgi.open_port = 0.0.0.0:5000
network.scgi.open_local = /session/rtorrent.sock

# Storage tuning
storage.disk_threads = 2
storage.max_open_files = 128
storage.hash_concurrency = 4
storage.load_threads = 4
```
