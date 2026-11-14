# XML-RPC & SCGI

`rstorrent` speaks the classic rTorrent wire protocol: **XML-RPC** and **JSON-RPC 2.0**, both transported over **SCGI**. This is the compatibility surface used by existing rTorrent tooling (ruTorrent, Flood, pyrocore, custom scripts). Both codecs dispatch into the same dynamic command map as [gRPC](gRPC-and-Remote-Control) and the [Web UI](WebUI), so every registered command is reachable from any interface.

---

## Enabling a Listener

SCGI is off by default. Open a listener via TOML, CLI overrides, or runtime commands:

```toml
[rpc]
scgi_port = 5000            # bare port binds 0.0.0.0 (all interfaces)
# scgi_socket = "~/.local/share/rstorrent/rpc.sock"
# scgi_token = "secret"     # bearer token required on TCP connections
```

```bash
# CLI overrides (executed after config load):
rstorrent -o scgi_port=5000
rstorrent -O "network.scgi.open_local=~/.local/share/rstorrent/rpc.sock"
```

| Command | Alias | Effect |
| --- | --- | --- |
| `network.scgi.open_port` | `scgi_port` | TCP listener. A bare port binds `0.0.0.0`; use `address:port` to choose the interface. |
| `network.scgi.open_local` | `scgi_local` | UNIX domain socket. `~` is expanded, a stale socket file is removed before binding, and the socket is created with mode `0600`. |
| `network.scgi.open_systemd` | — | Socket activation: uses the passed-in file descriptor (FD 3) when `LISTEN_PID` matches the process and `LISTEN_FDS > 0`. |

> **Security note:** a bare `scgi_port` binds **all interfaces**, and TCP connections are untrusted unless a token is configured. Prefer a UNIX socket for local frontends, or bind to loopback (`127.0.0.1:5000`) and set `scgi_token`.

---

## Wire Framing and Connection Limits

SCGI requests use netstring framing: `LENGTH:HEADERS,BODY`, where headers are NUL-separated key/value pairs and `CONTENT_LENGTH` is mandatory.

```text
70:CONTENT_LENGTH\00065\0SCGI\0001\0,<?xml version="1.0"?><methodCall>…</methodCall>
```

- Header block capped at **4096 bytes**; duplicate headers are rejected.
- Request body capped at **64 MiB**.
- **Keep-alive and pipelining** are supported; up to **100 concurrent connections** (`max_connections`, 0 disables the cap).
- Read, write, and idle timeouts: **60 s**; shutdown grace period: **1 s**.
- Responses carry HTTP-style headers (`Status`, `Content-Type`, `Content-Length`).
- Responses are **gzip-compressed** when enabled, the client accepts it, and the payload is ≥ 1024 bytes. Note: the `network.scgi.use_gzip` and `network.scgi.gzip.min_size` commands currently hold snapshot values — the handlers run with the built-in defaults.
- Commands execute on a blocking worker, so filesystem/fsync work never stalls the async runtime.
- SCGI connection counters are exported on the Prometheus `/metrics` endpoint (see `--metrics-addr`).

### Codec selection

The request `Content-Type` decides the codec:

| Content-Type | Codec |
| --- | --- |
| `application/json` | JSON-RPC 2.0 |
| `text/xml` | XML-RPC |
| anything else | Peek at the first non-whitespace body byte: `{` or `[` → JSON-RPC, otherwise XML-RPC |

---

## Authentication and Trust

- When `[rpc] scgi_token` is set, every request must carry `Authorization: Bearer <token>`. Authentication runs **before** codec parsing; failure returns a raw `Status: 401 Unauthorized`.
- **UNIX socket connections are trusted** (filesystem permissions are the gate). **TCP connections are untrusted** unless the bearer token matches.
- Untrusted connections may only call commands flagged as safe for public RPC; anything else is refused (XML-RPC fault `-507`).
- The trust level is decided by the listener, never by the client: the `UNTRUSTED_CONNECTION` header is ignored.

---

## XML-RPC

Supported value types: `i4`, `i8`, `int`, `boolean`, `double`, `dateTime.iso8601`, `base64`, `array`, `struct`. Map keys prefixed with `__` are filtered out of results.

### Target resolution

If the first parameter is a string, it is tried as an info-hash target (download, then file/tracker/peer targets), matching rTorrent's convention — e.g. `d.name` with the hash as the first argument.

### `system.multicall`

`system.multicall` is the only hardcoded method; every other name resolves through the command map. It takes an array of `{methodName, params}` structs and returns one result (or fault) per entry.

### Limits and dialects

| Setting | Command | Default | Range |
| --- | --- | --- | --- |
| Request size limit | `network.xmlrpc.size_limit` / `.set` | 64 MiB | 1024 bytes – 64 MiB |
| Response size cap | — | 100 MiB | fixed |
| Nesting depth | — | 128 | fixed |
| Integer dialect | `network.xmlrpc.dialect` / `.set` | `i8` (1) | `generic` (0), `i8` (1), `apache` (2) |

The dialect controls how integers are serialized: `generic` uses `<i4>`, `i8` uses `<i8>` (rTorrent default), `apache` uses `<int>`.

### Fault codes

| Code | Meaning |
| --- | --- |
| `-500` | Internal error |
| `-501` | Type mismatch |
| `-503` | Parse error |
| `-506` | No such method |
| `-507` | Refused — command not allowed for untrusted connections |
| `-509` | Size/limit exceeded |

---

## JSON-RPC 2.0 over SCGI

The same command map is exposed as JSON-RPC 2.0 **on the same SCGI listener** — there is no separate HTTP JSON endpoint.

- Single requests and **batches** are supported.
- **Notifications** (requests without `id`) produce no response.
- Floating-point numbers are rejected (the command map is integer/string based).
- Like XML-RPC, a first string parameter is tried as an info-hash target.
- Input limit: **100 MiB**.

| Error code | Meaning |
| --- | --- |
| `-32700` | Parse error |
| `-32600` | Invalid request |
| `-32601` | Method not found |
| `-32602` | Invalid params |
| `-32000` | Command execution error |

Example request body (sent inside an SCGI frame with `Content-Type: application/json`):

```json
{"jsonrpc": "2.0", "id": 1, "method": "d.name", "params": ["<info-hash>"]}
```

---

## Command Surface

The command map is dynamic — roughly **1000 registered names** across namespaces such as `d.*`, `f.*`, `p.*`, `t.*`, `network.*`, `throttle.*`, `session.*`, `storage.*`, `pieces.*`, `protocol.*`, `dht.*`, `schedule.*`, `ratio.*`, `choke_group.*`, `view.*`, `ui.*`, `system.*`, and `execute.*`.

- `system.listMethods` returns a frozen snapshot of all registered names (including itself).
- `system.methodHelp`, `system.methodSignature`, and `system.getCapabilities` are **not implemented**.
- New commands can be registered at runtime with the `method.insert*` family.
- See the [Configuration Guide](Configuration-Guide) for the `.rc` command syntax and namespace overview.

---

## Compatibility Notes

rTorrent clients probe a number of auxiliary methods. Most are implemented; a few are deliberate stubs or snapshots:

| Command | Behaviour |
| --- | --- |
| `network.port_open` | No-op (port state is managed by the engine) |
| `network.total_handshakes` | Always `0` |
| `network.scgi.dont_route` | Always `0` |
| `pieces.stats.total_size` | Always `0` |
| `pieces.sync.timeout_safe` | No-op |
| `pieces.hash.queue_size` | Approximate |
| `view.set` | Always throws (use `view.filter` / `view.sort` families) |
| `view.filter.temp.*` | State kept but not applied |
| `t.id` | Always `""` |
| `p.peer_rate`, `p.peer_total` | Approximations |
| `p.disconnect_delayed` | Disconnects immediately |
| `group.insert` | View argument validated but ignored |
| `protocol.pex.*`, `protocol.connection.*`, `protocol.choke_heuristics.*` | Stored, not wired into the engine |
| `network.http.*`, `rpc.use_xmlrpc`, `rpc.use_jsonrpc` | Snapshot values only |

Other behavioural notes:

- Ratio values use fixed-point integers: `200` means `2.00`.
- Throttle rates are capped at 4194303 KiB/s (`MAX_RATE_KB`).
- The `execute.*` family never spawns a shell; arguments are passed directly (max 128 args, `~/` tilde expansion only).
- Path-returning commands support encoding suffixes: `.hex`, `.base64`, `.base64_as_binary`, `.as_binary`, `.or_base64`, `.or_as_binary`, plus `realpath.or_empty` / `realpath.or_throw`.
- `d.custom1`–`d.custom5` are persisted per download; `d.custom1` is the conventional label slot.

---

## Related Pages

- [gRPC & Remote Control](gRPC-and-Remote-Control) — native high-performance interface
- [Web UI](WebUI) — built-in browser interface
- [Configuration Guide](Configuration-Guide) — `[rpc]` section, `.rc` syntax, command namespaces
- [CLI Reference](CLI-Reference) — `-o` / `-O` overrides
