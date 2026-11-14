# Configuration Guide

`rstorrent` supports three configuration layers:

1. **Modern `rstorrent.toml`** — structured TOML tables for network, storage, protocol, RPC, DHT, logging, metrics and the native Web UI.
2. **Classic `.rtorrent.rc`** — rTorrent-compatible command scripting. Every line is a command executed against the same command map that serves XML-RPC/SCGI, so any registered `name.set` command is a valid configuration key.
3. **Lua** — an optional `rtorrent.lua` file located next to the active config file is loaded automatically at startup (see [Lua scripting](#lua-scripting)).

---

## File Lookup Order

The first existing file wins. The search is skipped entirely with `-n`.

1. `./rstorrent.toml` (current working directory)
2. `$XDG_CONFIG_HOME/rstorrent/rstorrent.toml`
3. `$XDG_CONFIG_HOME/rstorrent/rstorrent.rc`
4. `$XDG_CONFIG_HOME/rtorrent/rtorrent.rc`
5. `~/.config/rstorrent/rstorrent.toml`
6. `~/.rstorrent.toml`
7. `~/.config/rstorrent/rstorrent.rc`
8. `~/.config/rtorrent/rtorrent.rc`
9. `~/.rstorrent.rc`
10. `~/.rtorrent.rc`
11. `/config/rstorrent.toml`, `/config/.rstorrent.rc`, `/config/.rtorrent.rc`, `/config/rstorrent.rc`, `/config/rtorrent.rc` (container paths)
12. `/etc/rstorrent/rstorrent.toml`, `/etc/rstorrent/rstorrent.rc`, `/etc/rtorrent/rtorrent.rc`

Override the path explicitly with `-c`:

```bash
rstorrent -c /path/to/rstorrent.toml
rstorrent -c /path/to/.rtorrent.rc
```

Run without any configuration file:

```bash
rstorrent -n
```

Note that `-n` also skips TOML-only features (metrics server, Web UI, log file), because they are configured exclusively through TOML keys.

## Override Precedence

Later layers win:

1. Config file (TOML or `.rc`)
2. Built-in CLI mappings: `-b`, `-i`, `-p`, `--proxy`, `-d`, `-s`, `--grpc-port`, `--grpc-socket`
3. `-o key=value` entries, in order
4. `-O '<command line>'` entries, in order

See the [CLI Reference](CLI-Reference) for the full flag list.

---

## TOML Reference (`rstorrent.toml`)

All keys are optional; an unset key keeps its runtime default.

### `[network]`

| Key | Type | Default | Effect |
|---|---|---|---|
| `port_range` | string | `"6881-6999"` | Listen port range (`first-last`). `0-0` disables the listener. |
| `port` | integer | — | Single listen port; **wins over `port_range`**. |
| `bind_address` | string | unspecified | Bind address for the peer listener. |
| `local_address` | string | unspecified | Local address for outgoing connections. |
| `enable_upnp` | bool | `true` | UPnP IGD / NAT-PMP automatic port mapping. Read once when the listener starts. |
| `enable_utp` | bool | `true` | uTP (BEP 29) support. Read once when the listener starts. |
| `proxy` | string | none | Outbound peer proxy: `socks5://`, `socks5h://`, `http://`, `https://`. |
| `max_upload_rate` | integer (bytes/s) | `0` (unlimited) | Global upload cap. |
| `max_download_rate` | integer (bytes/s) | `0` (unlimited) | Global download cap. |
| `download_throttle_kb` | integer (KiB/s) | `0` (unlimited) | Global download cap, KiB/s unit. |
| `upload_throttle_kb` | integer (KiB/s) | `0` (unlimited) | Global upload cap, KiB/s unit. |
| `max_uploads` | integer | `0` | Global upload slots. |
| `max_downloads` | integer | `0` | Global download slots. |
| `max_peers` | integer | `0` | Global peer cap (leeching). |
| `max_peers_seed` | integer | `0` | Global peer cap (seeding). |

### `[storage]`

| Key | Type | Default | Effect |
|---|---|---|---|
| `directory` | string | `"."` | Default download directory. |
| `session` (alias `session_directory`) | string | fallback chain below | Session directory for resume data. |
| `watch_directory` | string | none | Auto-loads and starts `.torrent` files dropped here (non-recursive, live monitoring). CLI `-w` wins. |
| `disk_threads` | integer | cores ÷ 2, clamped 2–16 (fallback 4) | Disk worker count, 1–64. Fixed after the disk pool is created. |
| `max_open_files` | integer | `64` per worker | Per-worker open-file cache, 4–65536. Fixed after the disk pool is created. |

### `[protocol]`

| Key | Type | Default | Effect |
|---|---|---|---|
| `encryption` | string | allow / allow | Encryption policy. A single mode (`deny`, `allow`, `prefer`, `require`) applies to both layers; two comma-separated values set `handshake,stream`. Legacy rTorrent tokens (`none`, `allow_incoming`, `try_outgoing`, `enable_retry`, `prefer_plaintext`, `require_rc4`) are also accepted. |
| `peer_id` | string | generated | 40-character hex string, or a raw string padded/truncated to 20 bytes. |

### `[rpc]`

| Key | Type | Default | Effect |
|---|---|---|---|
| `scgi_port` | integer | closed | SCGI/XML-RPC TCP port. A bare port binds `0.0.0.0`. |
| `scgi_socket` | string | closed | SCGI Unix socket (`~/` expanded; created with mode `0600`). |
| `grpc_port` | integer | closed | gRPC TCP port. A bare port binds `127.0.0.1` (loopback). |
| `grpc_socket` | string | closed | gRPC Unix socket (`~/` expanded). |
| `scgi_token` | string | none | Bearer token required of SCGI callers. Empty disables auth. Installed before listeners open. |
| `grpc_token` | string | none | Bearer token required of gRPC callers. Empty disables auth. Installed before listeners open. |
| `grpc_tls_cert` | string | none | PEM certificate for gRPC TLS. Requires `grpc_tls_key`. |
| `grpc_tls_key` | string | none | PEM private key for gRPC TLS. Requires `grpc_tls_cert`. |

### `[dht]`

| Key | Type | Default | Effect |
|---|---|---|---|
| `mode` | string | `"auto"` | `disable`/`off`, `auto` (enabled when a torrent needs it), `on`. |
| `port` | integer | `6881` | DHT UDP port (dual-stack IPv4/IPv6). |

### `[logging]`

| Key | Type | Default | Effect |
|---|---|---|---|
| `level` | string | `"info"` | Maximum tracing level: `trace`, `debug`, `info`, `warn`, `error`, `off`. |
| `file` | string | `<temp dir>/rstorrent.log` | Log file path. |

### `[metrics]`

| Key | Type | Default | Effect |
|---|---|---|---|
| `addr` | string | disabled | Bind address of the Prometheus `/metrics` HTTP server (e.g. `127.0.0.1:9090`). CLI `--metrics-addr` overrides. |

### `[webui]`

| Key | Type | Default | Effect |
|---|---|---|---|
| `addr` | string | disabled | Bind address of the native Web UI (e.g. `127.0.0.1:8080`). CLI `--webui-addr` overrides. |
| `token` | string | none | Bearer token required of Web UI callers. An empty token disables authentication (callers stay untrusted and restricted to read-only commands). CLI `--webui-token` overrides. |
| `tls_cert` | string | none | PEM certificate for Web UI HTTPS. Requires `tls_key`. TOML-only (no CLI flag). |
| `tls_key` | string | none | PEM private key for Web UI HTTPS. Requires `tls_cert`. TOML-only. |

### Complete example

```toml
[network]
port = 50000
bind_address = "0.0.0.0"
enable_utp = true
enable_upnp = true
# proxy = "socks5h://127.0.0.1:1080"
download_throttle_kb = 0        # unlimited
upload_throttle_kb = 2500
max_uploads = 8
max_downloads = 16

[storage]
directory = "~/Downloads"
session = "~/.local/share/rstorrent/session"
watch_directory = "~/Downloads/watch"
disk_threads = 4
max_open_files = 128

[protocol]
encryption = "allow"

[rpc]
grpc_port = 50051
grpc_socket = "/tmp/rstorrent-grpc.sock"
# grpc_token = "change-me"
scgi_port = 5000
scgi_socket = "/tmp/rstorrent-scgi.sock"

[dht]
mode = "auto"
port = 6881

[logging]
level = "info"
file = "~/.local/share/rstorrent/rstorrent.log"

[metrics]
addr = "127.0.0.1:9090"

[webui]
addr = "127.0.0.1:8080"
token = "change-me"
```

---

## Legacy `.rtorrent.rc` Reference

Each line is a command: `name = arguments`. `#` starts a comment, `;` separates commands, a trailing `\` continues a line, and `$command` is evaluated inside arguments.

```ini
directory.default.set = /downloads
session.path.set = /session
network.port_range.set = 50000-50000
dht.port.set = 6881
network.scgi.open_port = 0.0.0.0:5000
network.scgi.open_local = /session/rtorrent.sock

# Storage tuning (read once at startup)
storage.disk_threads = 4
storage.max_open_files = 128
storage.hash_concurrency = 4
storage.load_threads = 4
```

Configuration keys are registered commands. The most useful namespaces:

| Namespace | Highlights |
|---|---|
| `network.*` | `port_range`, `bind_address`, `local_address`, `proxy.global`, `block.ipv4/ipv6/udp/incoming/outgoing`, `prefer.ipv6`, `listen.backlog`, `send_buffer.size`, `receive_buffer.size`, `tos`, `max_open_files`, `max_open_sockets`. Most listener keys are snapshot-only: they apply when the listener starts, not live. |
| `network.scgi.*` / `network.grpc.*` | `open_port`, `open_local` (aliases `scgi_port`, `scgi_local`, `grpc_port`, `grpc_local`), `network.scgi.open_systemd` (socket activation), `network.xmlrpc.size_limit` (default 64 MiB), `network.xmlrpc.dialect` (`generic`, `i8`, `apache`). |
| `throttle.*` | `global_down.max_rate` / `global_up.max_rate` with `.set` (bytes/s) and `.set_kb` (KiB/s); named throttles via `throttle.down = name,kb` / `throttle.up = name,kb`; `throttle.names`; `max_uploads`, `max_downloads`, `max_peers.normal`, `max_peers.seed`. `0` = unlimited. Assign per torrent with `d.throttle_name.set`. |
| `session.*` | `path`, `name`, `use_lock` (default on), `use_fsyncdisk` (default on, locked after startup), `on_completion`, actions `session.save` and `session.checkpoint`. |
| `storage.*` | `disk_threads`, `max_open_files`, `hash_concurrency` (default min(4, cores)), `hash_read_cache` (default 64), `load_threads` (default min(4, cores)). Fixed after the pool/first recheck; restart to change. |
| `protocol.*` | `encryption` (`.handshake`, `.stream`: `deny`/`allow`/`prefer`/`require`), `pex` (default on), `connection.leech` / `connection.seed`, `choke_heuristics.up/down.leech/seed`. |
| `pieces.*` | `memory.max` (byte suffixes `k/m/g/t` accepted, `0` = unlimited), `preload.min_size`, `preload.min_rate`, `preload.type`, `sync.timeout`, `sync.always_safe`, `hash.on_completion`, read-only memory/sync counters. |
| `dht.*` | `mode` (`disable`/`off`/`auto`/`on`), `port`, `override_port`, `add_node`, `bootstrap`, `refresh`, `bootstrap_nodes`, `statistics`, `reset_statistics`, `refresh_interval` (default 900 s), `bootstrap_retry_interval` (default 60 s). |
| `schedule` / `scheduler.*` | `schedule = name,absolute,interval,command`, `schedule.if_absent`, `schedule.remove`, `scheduler.max_active` (default −1 = unlimited active torrents). |
| `ratio.*` | `ratio.enable` / `ratio.disable`, `ratio.min` (default 200 = 2.00), `ratio.max` (default 300 = 3.00), `ratio.upload` (default 20 MiB), `ratio.command` (action when the ratio is reached). Group-aware forms: `group.seeding.ratio.*`. Fixed-point convention: 200 = 2.00. |
| `choke_group.*` | `up.max` / `down.max` (default 50 slots, −1 = unlimited), `up.min` / `down.min`, `up.heuristics` / `down.heuristics`, `tracker.mode` (`normal`/`aggressive`), group management (`insert`, `remove`, `list`). |
| `log.*` | `log.open_file`, `log.append_file`, gz variants, `log.add_output`, `log.execute`, `log.rpc` (alias `log.xmlrpc`). |
| `system.*` | `system.file.allocate`, `system.file.max_size`, `system.umask.set`, `system.shutdown.normal` / `.quick`, `system.env`, `system.cwd`, socket budgets (`system.sockets.*`). |
| `ui.*` / `keys.*` | `ui.keymap.style` (`default`/`vi`/`emacs`), `keys.layout` (`qwerty`/`azerty`/`qwertz`/`dvorak`), `ui.throttle.global.step.small/medium/large`, `ui.input.history.size`, `ui.color.*`. |
| `load.*`, `d.*`, `f.*`, `p.*`, `t.*`, `view.*` | Torrent/file/peer/tracker/view-scoped commands, also usable from scripts and RPC. |

The full registered command list is available at runtime via `system.listMethods` (also over XML-RPC).

### Startup defaults

Unless overridden, the daemon installs these schedules:

- `session_save` — every 1200 s (`session.save`)
- `session_checkpoint` — every 300 s (`session.checkpoint`, resume-only)
- `low_diskspace` — `5,60,close_low_diskspace=500M`

Default views: `main`, `default`, `name`, `active`, `started`, `stopped`, `complete`, `incomplete`, `hashing`, `seeding`, `leeching`.

---

## Lua Scripting

If `rtorrent.lua` exists in the same directory as the active config file, it is loaded automatically at startup. Two commands are also available from `.rc` files and RPC:

```ini
lua.execute = /path/to/script.lua
lua.execute.str = "rtorrent.call('directory.default.set', '/downloads')"
```

An embedded prelude binds `rtorrent.call(command, args...)` to the live command map, so Lua scripts can drive the full command surface.

---

## Logging

- `[logging] level` caps the global tracing level (default `info`).
- `RUST_LOG` adds per-target `EnvFilter` directives on top; both the level cap and the filter must pass (e.g. `RUST_LOG=lt_dht=debug,info`).
- In interactive (TUI) mode, logs are written to `[logging] file` (default `<temp dir>/rstorrent.log`) so the terminal is not corrupted. In daemon mode, logs go to stderr unless a file is configured.
- `RSTORRENT_METRICS_FILE` enables a JSONL event recorder (startup/shutdown/phase events) at the given path.

## Environment Variables

| Variable | Effect |
|---|---|
| `RUST_LOG` | Per-target tracing filter, combined with `[logging] level`. |
| `RSTORRENT_METRICS_FILE` | Path for the JSONL metrics recorder; unset/empty disables it. |
| `XDG_CONFIG_HOME` | Config search root. |
| `XDG_DATA_HOME` | Default session dir root (`$XDG_DATA_HOME/rstorrent/session`). |
| `HOME` | Config fallbacks, `~/` expansion in socket/path settings, session fallback. |

## Session Directory

Fallback chain: `-s` → configured `session.path` → `$XDG_DATA_HOME/rstorrent/session` → `~/.local/share/rstorrent/session` → `./.session`.

The session directory is locked; a second instance using the same directory aborts at startup. The DHT node cache is persisted at `<session>/dht.cache`.

---

## Compatibility Notes

- CLI flags `-D`, `-I`, `-K` are accepted for rTorrent compatibility but have no effect.
- The rTorrent `import=` command is not supported; use `-c`, `-O` or Lua instead.
- Some legacy keys are stored for compatibility but not consumed by the engine: `network.http.*` tuning keys, `network.rpc.use_xmlrpc` / `use_jsonrpc`, `network.scgi.use_gzip`, `file.prioritize_toc*`, `system.daemon` (use `--daemon`).
- The legacy `ui.*` / `keys.*` keymap variables (`ui.keymap.style`, `keys.layout`, colors, throttle steps, history size) are maintained as RPC state for compatibility; the shipped terminal surfaces use their own keymaps. The standalone client's throttle layout is set via `keys_layout` in its own `config.toml`, not the daemon's `keys.layout`. See [Keyboard Shortcuts](Keyboard-Shortcuts).
- `network.port_open`, `pieces.sync.timeout_safe` and `log.vmmap.dump` are accepted as no-ops.
- Keys fixed after startup: `storage.disk_threads`, `storage.max_open_files`, `storage.hash_concurrency` (after first recheck), `session.use_fsyncdisk`, `enable_utp` / `enable_upnp`.
