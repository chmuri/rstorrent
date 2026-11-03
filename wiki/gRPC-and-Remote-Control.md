# gRPC & Remote Control

`rstorrent` provides a native, high-performance gRPC interface in [`crates/rt-grpc`](../crates/rt-grpc) (`rtorrent.v1` package) powered by Tonic and Protocol Buffers.

---

## Running in Daemon Mode

On seedboxes or headless servers, launch `rstorrent` with `--daemon`:

```bash
# Listen on TCP port 50051:
rstorrent --daemon --grpc-port 50051 -d ~/Downloads -s ~/.local/share/rstorrent/session

# Or listen on a local UNIX domain socket (recommended for local frontends):
rstorrent --daemon --grpc-socket /tmp/rstorrent.sock
```

---

## Standalone Terminal Client (`rstorrent-tui`)

The workspace includes a standalone terminal client that connects to a running daemon over gRPC:

```bash
# Connect to daemon over TCP:
rstorrent-tui --endpoint 127.0.0.1:50051

# Connect to daemon over UNIX socket:
rstorrent-tui --endpoint unix:///tmp/rstorrent.sock
```

The standalone client provides the exact same rich Ratatui interface, mouse controls, and keybindings without hosting the BitTorrent network engine in the UI process.

---

## Available gRPC Services

### `TorrentService`
- `ListTorrents`: Returns full snapshots of all downloads with transfer rates, ETA, pieces, and error states.
- `GetTorrent`: Fetches detailed properties for a single torrent.
- `AddTorrent`: Adds a new download from `.torrent` bytes, file path, or magnet URI.
- `ControlTorrent`: Start, stop, pause, or resume downloads.
- `RemoveTorrent`: Removes a download with optional disk deletion.
- `SetTorrentPriority`: Adjusts swarm bandwidth priority.
- `StreamEvents`: Server-streaming subscription delivering real-time download and peer events.

### `SystemService`
- `GetVersion`: Retrieves daemon version, protocol identifiers, and uptime.
- `GetSessionStats`: Global bandwidth throughput, verified chunk rates, and disk space (`statvfs`).
- `StreamStats`: Periodic streaming telemetry.
- `Shutdown`: Requests clean daemon termination and state flush.

### `CommandService`
- `Execute`: Evaluates classic rTorrent commands (`d.name`, `network.port_range.set`, etc.).
- `Multicall`: Executes batch multi-target commands with atomic result arrays.
