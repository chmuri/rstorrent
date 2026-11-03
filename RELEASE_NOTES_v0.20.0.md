# rtorrent v0.20.0 — Modern Rust BitTorrent Engine & Client (Beta Release)

We are thrilled to announce **rstorrent v0.20.0**, a milestone release transforming `rstorrent` into a production-grade, full-featured BitTorrent client with complete protocol modernizations, autonomous port mapping, and next-generation architecture.

---

## What's New in v0.20.0

### 🚀 BEP 29 Micro Transport Protocol (uTP) & LEDBAT Congestion Control
- **Full uTP Stack**: Complete implementation of BEP 29 over UDP with 20-byte packet framing (`ST_DATA`, `ST_FIN`, `ST_STATE`, `ST_RESET`, `ST_SYN`) and Selective ACKs (SACK).
- **LEDBAT Algorithm**: Implemented Low Extra Delay Background Transport with dynamic queuing delay target ($100\,\text{ms}$), base delay min-filtering, sliding congestion windows, and RFC 6298 Jacobson-Karels RTT/RTO timers.
- **Tokio Stream Integration**: `UtpStream` implements Tokio `AsyncRead` and `AsyncWrite` with out-of-order reassembly buffers and in-flight retransmission queues.
- **UDP Multiplexer**: `UtpSocket` multiplexes inbound and outbound uTP peer sessions transparently alongside TCP.

### ⚙️ Modern `rstorrent.toml` Configuration Format
- **Human-Readable TOML**: Introduced structured configuration (`[network]`, `[storage]`, `[protocol]`, `[rpc]`, `[dht]`).
- **XDG Standards**: Automatic resolution searching `./rstorrent.toml`, `$XDG_CONFIG_HOME/rstorrent/rstorrent.toml`, and `~/.config/rstorrent/rstorrent.toml`.
- **Full Compatibility**: Seamless fallback to classic `.rtorrent.rc` scripts and commands.

### 🌐 Automatic Router Port Forwarding (UPnP IGD / NAT-PMP)
- **Autonomous SSDP Discovery**: Automatic SSDP M-SEARCH broadcast finding Internet Gateway Devices on local networks.
- **SOAP Port Forwarder**: Automatically opens inbound TCP and UDP/uTP listen ports on home routers in a background loop without manual port forwarding rules.

### 📂 Real-Time Watch Directory Monitoring
- **Linux inotify / notify Integration**: Replaced startup one-off scans with continuous filesystem monitoring.
- **Zero-Poll Auto Ingestion**: Dropping `.torrent` files into the watch directory instantly loads, parses, and begins downloading in real time.

### ⚡ Lock-Free Asynchronous Disk I/O & Tit-for-Tat Choking
- **Non-Blocking Upload Reads**: Peer upload block requests are fulfilled asynchronously from background disk workers without holding the engine `Download` mutex, eliminating swarm stuttering.
- **Tit-for-Tat Choking Supervisor**: Standard BitTorrent choking algorithm running 10-second evaluation rounds (favoring highest uploaders while leeching and highest downloaders while seeding) and 30-second rotating optimistic unchoking.
- **Endgame Mode Global CANCEL**: Instant broadcast of `CANCEL` packets to all peers in the swarm upon block completion.

### 🖥️ Next-Generation Terminal UI & gRPC Enhancements
- **Dynamic Filesystem Metrics**: Real `statvfs` disk capacity tracking.
- **Interactive Priority & Throttles**: File and torrent priority toggling (`+`/`-`), and live throttle hotkeys (`a`/`z` for upload, `A`/`Z` for download).
- **Complete gRPC Inspection**: Populated file hierarchies and connected peer snapshots in `rtorrent.v1` services.

---

## Getting Started

### Run with TOML configuration:
```bash
rstorrent -c ~/.config/rstorrent/rstorrent.toml
```

### Run as a headless daemon:
```bash
rstorrent --daemon --grpc-port 50051 -d ~/Downloads -w ~/Downloads/watch
```

### Connect with standalone TUI:
```bash
rstorrent-tui --endpoint 127.0.0.1:50051
```

---

## Verifications & Quality Assurance

- **Unit & Integration Tests**: 100% passed across all 15 crates (`cargo test --workspace`).
- **Static Analysis**: Clean `cargo clippy --workspace --all-targets -- -D warnings` (zero warnings).
- **Format**: 100% compliant with Rust 2024 `cargo fmt`.
- **Binaries**: Standalone, optimized release binaries for Linux and macOS.
