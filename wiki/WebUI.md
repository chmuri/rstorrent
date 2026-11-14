# Web UI

`rstorrent` ships a built-in browser interface: an embedded single-page app served by an axum HTTP server, backed by a JSON REST API and a WebSocket event stream. It exposes the same torrent/session data and the same command map as [gRPC](gRPC-and-Remote-Control) and [XML-RPC/SCGI](XML-RPC-and-SCGI).

The Web UI is **opt-in and disabled by default** — a standard install opens no extra port.

---

## Enabling

```bash
rstorrent --daemon --webui-addr 127.0.0.1:8080 --webui-token "secret"
```

or in `rstorrent.toml`:

```toml
[webui]
addr = "127.0.0.1:8080"
token = "secret"
# tls_cert = "/etc/rstorrent/webui.crt"   # both required to enable TLS
# tls_key = "/etc/rstorrent/webui.key"
```

- The SPA is served at `/` (embedded in the binary — no external assets or build step).
- An invalid `addr` logs a warning and disables the Web UI; it never blocks daemon startup.
- Shutdown drains in-flight requests for up to 5 s.

---

## Authentication and Trust

The `/api` subtree is gated by the same bearer-token middleware and trust model as the other RPC surfaces:

- Send `Authorization: Bearer <token>`. For the browser WebSocket (which cannot set headers), pass `?token=<token>` as a query parameter instead.
- A **valid token** makes the connection **trusted** (full command access).
- When a token is configured, requests with a missing or wrong token get **401**.
- When **no token is configured**, requests are allowed but **untrusted**: only commands flagged safe for public RPC can execute. Treat this as a read-only-ish mode and always set a token on non-loopback binds.

---

## REST API

All endpoints live under `/api` and speak JSON.

| Method | Path | Description |
| --- | --- | --- |
| GET | `/api/torrents` | Array of torrent summaries (all downloads). |
| GET | `/api/torrents/{hash}` | One summary by hex info-hash. `400` for an invalid hash, `404` if not found. |
| POST | `/api/torrents/{hash}/start` | Runs `d.start` on the torrent. |
| POST | `/api/torrents/{hash}/stop` | Runs `d.stop`. |
| POST | `/api/torrents/{hash}/remove` | Runs `d.erase` (removes from session; does not delete data). |
| POST | `/api/torrents/{hash}/recheck` | Runs `d.check_hash`. |
| POST | `/api/command` | Generic command invocation (see below). |
| GET | `/api/stats` | Session-wide statistics snapshot. |
| GET | `/api/version` | Version/build metadata. |
| GET | `/api/events` | WebSocket upgrade → live event stream. |

The four action endpoints return the same `CommandResponse` shape as `/api/command`.

### Generic command endpoint

`POST /api/command` body:

```json
{
  "command": "d.name",
  "target": "0123456789abcdef0123456789abcdef01234567",
  "args": []
}
```

- `command` — any registered command name (same map as XML-RPC/gRPC).
- `target` — hex info-hash; empty string means no target. An invalid hash returns `success: false` with an error message.
- `args` — JSON values: zero args → empty argument, one arg → single argument, several → argument list.

Response:

```json
{ "success": true, "result": "My Torrent", "error": "" }
```

Commands run on a blocking worker so filesystem/fsync work never stalls the server. JSON ↔ engine conversion notes: booleans become integers, non-integer numbers become strings, and binary values that are not valid UTF-8 are base64-encoded on the way out.

### Torrent summary fields

`GET /api/torrents` returns objects with:

| Field | Type | Meaning |
| --- | --- | --- |
| `hash` | string | Hex info-hash |
| `name` | string | Torrent name |
| `size_bytes` / `completed_bytes` | int | Total / verified bytes |
| `up_rate` / `down_rate` | int | Current rates, bytes/s |
| `up_total` / `down_total` | int | Session totals, bytes |
| `state` | string | Engine state (e.g. `Downloading`, `Seeding`, `Paused`) |
| `is_active` / `is_complete` / `is_hash_checking` | bool | State flags |
| `priority` | int | 0 off, 1 low, 2 normal, 3 high |
| `directory` | string | Save path |
| `category` | string | Value of `d.custom1` (label slot) |
| `message` | string | Status/error message |
| `state_changed` / `started` / `finished` | int | Unix timestamps (seconds) |

### Session stats fields

`GET /api/stats` returns: `up_rate`, `down_rate`, `up_total`, `down_total`, `active_torrents`, `total_torrents`, `listen_port`, `throttle_up_limit`, `throttle_down_limit`, `dht_active`, `dht_nodes`, `dht_torrents`, `dht_port`.

### Version fields

`GET /api/version` returns: `version` (crate version), `api_version` (library/API version), `build_target` (architecture), `uptime_seconds`.

---

## WebSocket Event Stream

Connect to `GET /api/events` (with `?token=<token>` when a token is configured). Every message is a JSON object tagged with `type`:

```json
{ "type": "torrent", "event_name": "started", "hash": "…", "summary": { … }, "timestamp": 1763078400 }
```

```json
{ "type": "stats", "stats": { … }, "timestamp": 1763078400 }
```

- **`torrent`** — one message per download-list event (start/stop/metadata/finished/…), carrying the same summary object as REST.
- **`stats`** — a session-stats snapshot sent **immediately on connect**, then **every second**, so a browser can refresh totals without polling.
- Events come from a 256-slot broadcast channel; a slow client that falls behind **skips missed events** rather than stalling the stream — the next stats tick resyncs totals.
- `timestamp` values are Unix seconds.

---

## TLS

TLS is configured **only via TOML** (`[webui] tls_cert` + `tls_key`, PEM, rustls). Both paths are required; a partial configuration logs a warning and serves plain HTTP. There are no CLI flags for Web UI TLS.

---

## Examples

```bash
TOKEN="secret"
BASE="http://127.0.0.1:8080"

# List torrents:
curl -H "Authorization: Bearer $TOKEN" "$BASE/api/torrents"

# Start a torrent:
curl -X POST -H "Authorization: Bearer $TOKEN" \
  "$BASE/api/torrents/<info-hash>/start"

# Run an arbitrary command:
curl -X POST -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"command":"throttle.global_up.max_rate","target":"","args":[]}' \
  "$BASE/api/command"

# Session stats:
curl -H "Authorization: Bearer $TOKEN" "$BASE/api/stats"
```

WebSocket clients (browser): `new WebSocket("ws://127.0.0.1:8080/api/events?token=secret")`.

---

## Security Notes

- Bind to loopback unless you need remote access; always set a token on reachable interfaces.
- Without a token the API still answers, but only the untrusted-safe command subset executes — do not rely on this as an access control.
- The trust model matches gRPC/SCGI: the server decides trust from the listener and credentials, never from client-supplied claims.

---

## Related Pages

- [gRPC & Remote Control](gRPC-and-Remote-Control)
- [XML-RPC & SCGI](XML-RPC-and-SCGI)
- [Configuration Guide](Configuration-Guide) — `[webui]` section reference
- [CLI Reference](CLI-Reference) — `--webui-addr`, `--webui-token`
