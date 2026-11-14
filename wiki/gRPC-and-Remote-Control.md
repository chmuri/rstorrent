# gRPC & Remote Control

`rstorrent` provides a native, high-performance gRPC interface in [`crates/rt-grpc`](../crates/rt-grpc) (package `rtorrent.v1`, defined in `crates/rt-grpc/proto/rstorrent.proto`) powered by Tonic and Protocol Buffers. It exposes three services — `TorrentService`, `SystemService`, and `CommandService` — covering torrent management, session telemetry, and the full classic rTorrent command surface.

For the classic XML-RPC/JSON-RPC wire protocol see [XML-RPC & SCGI](XML-RPC-and-SCGI); for the built-in browser interface see [Web UI](WebUI).

---

## Running in Daemon Mode

On seedboxes or headless servers, launch `rstorrent` with `--daemon`:

```bash
# Listen on TCP port 50051 (loopback by default):
rstorrent --daemon --grpc-port 50051 -d ~/Downloads -s ~/.local/share/rstorrent/session

# Or listen on a local UNIX domain socket (recommended for local frontends):
rstorrent --daemon --grpc-socket /tmp/rstorrent.sock
```

Listeners can also be opened at runtime with the `network.grpc.open_port` (alias `grpc_port`) and `network.grpc.open_local` (alias `grpc_local`) commands, or configured in `rstorrent.toml`:

```toml
[rpc]
grpc_port = 50051          # bare port binds 127.0.0.1
# grpc_socket = "/tmp/rstorrent.sock"
# grpc_token = "secret"    # bearer token required on TCP connections
# grpc_tls_cert = "/etc/rstorrent/tls.crt"
# grpc_tls_key = "/etc/rstorrent/tls.key"
```

### Connection security

- **TCP**: a bare port number binds to `127.0.0.1`. Use `address:port` to bind elsewhere. Connections are *untrusted* unless `[rpc] grpc_token` is configured and the client presents a matching `authorization: Bearer <token>` metadata header.
- **UNIX socket**: created with mode `0600` (owner-only), a stale socket file is removed before binding, and the socket is removed on exit. Local socket connections are *trusted*.
- **TLS**: optional, enabled by setting both `grpc_tls_cert` and `grpc_tls_key` (PEM, rustls).
- **Message limits**: 64 MiB maximum per message in both directions.
- **Trust model**: the trust level is decided by the listener, never by the client. The `trusted` field on `CommandRequest` is ignored — untrusted connections are restricted to the safe command subset regardless of what they claim.

---

## Standalone Terminal Client (`rstorrent-tui`)

The workspace includes a standalone terminal client that connects to a running daemon over gRPC:

```bash
# Connect to daemon over TCP:
rstorrent-tui --endpoint 127.0.0.1:50051

# Connect to daemon over UNIX socket:
rstorrent-tui --endpoint unix:///tmp/rstorrent.sock
```

The standalone client provides the exact same rich Ratatui interface, mouse controls, and keybindings without hosting the BitTorrent network engine in the UI process. See [CLI Reference](CLI-Reference) for its configuration file and themes.

---

## Available gRPC Services

### `TorrentService`

| RPC | Description |
| --- | --- |
| `ListTorrents` | Snapshots of downloads with transfer rates, ETA, pieces, and error states. Optional view filter (an unknown view name falls back to all torrents) and info-hash filter (hashes are lowercased). |
| `GetTorrent` | Detailed properties for a single torrent: summary plus piece bitfield, file list, trackers, and peers. Returns `NOT_FOUND` for an unknown hash. |
| `AddTorrent` | Adds a download from one of: `torrent_file` (raw `.torrent` bytes), `file_path`, `url`, or `magnet_uri`. Optional `destination_directory` and `start_now`. See caveats below. |
| `ControlTorrent` | `START`, `STOP`, `PAUSE`, `RESUME`, `CLOSE`, or `CHECK_HASH` actions. |
| `RemoveTorrent` | Removes a download from the session. The `delete_data` flag is currently ignored — on-disk data is never deleted. |
| `SetTorrentPriority` | Sets swarm bandwidth priority: `0` off, `1` low, `2` normal, `3` high. |
| `BulkControlTorrents` | Applies one control action to many torrents. |
| `BulkRemoveTorrents` | Removes many torrents in one call. |
| `BulkSetTorrentPriority` | Sets priority for many torrents in one call. |
| `StreamEvents` | Server-streaming subscription delivering real-time download-list events. Backed by a 256-slot broadcast channel; a slow client that falls behind skips missed events rather than stalling the stream. An optional `event_filter` restricts which event names are delivered. |

### `SystemService`

| RPC | Description |
| --- | --- |
| `GetVersion` | Daemon version, library/API version, build architecture, and uptime. |
| `GetSessionStats` | Global upload/download rates and totals, active torrent count, DHT statistics, listen port, and global throttle limits. |
| `StreamStats` | Periodic stats streaming; `interval_ms` defaults to 1000 (a value of 0 is treated as 1000). |
| `Shutdown` | Requests clean daemon termination and state flush; a `force` flag skips the graceful path. |

### `CommandService`

| RPC | Description |
| --- | --- |
| `Execute` | Evaluates any registered rTorrent command (`d.name`, `network.port_range.set`, `throttle.global_up.max_rate.set`, …) against the same dynamic command map used by XML-RPC. `target_hash` selects a download target; the `args` list maps to a single argument or an argument list. Execution runs on a blocking worker so filesystem/fsync work never stalls the async runtime. |
| `Multicall` | Executes a batch of commands sequentially and returns one result per entry. |
| `ListCommands` | Returns the names of all registered commands. |

---

## Caveats

- **`AddTorrent` URL source is not supported**: passing `url` returns `success = false` with a message asking for `torrent_file` bytes instead. Magnet URIs work.
- **`AddTorrentRequest.initial_commands` is not applied** — set properties with follow-up `Execute` calls instead.
- **`RemoveTorrent.delete_data` is ignored**: removal never deletes files from disk. Use `Execute` with the appropriate `d.*` commands if you need data removal semantics.
- **`CommandRequest.trusted` is ignored**: trust is decided by the listener (see above).
- **`TorrentSummary.category`** carries the value of the `d.custom1` field (the conventional rTorrent label slot).

---

## Related Pages

- [XML-RPC & SCGI](XML-RPC-and-SCGI) — classic rTorrent wire compatibility (ruTorrent, Flood, pyrocore, …)
- [Web UI](WebUI) — built-in browser interface (REST + WebSocket)
- [Configuration Guide](Configuration-Guide) — `[rpc]` section reference
- [CLI Reference](CLI-Reference) — `--daemon`, `--grpc-port`, `--grpc-socket`, `rstorrent-tui`
