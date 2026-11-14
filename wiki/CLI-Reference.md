# CLI Reference

Two binaries ship with the project:

- **`rstorrent`** — the client/daemon (packaging also installs an `rtorrent` alias with identical behavior).
- **`rstorrent-tui`** — a standalone terminal client that connects to a running daemon over gRPC.

---

## `rstorrent`

```
rstorrent [OPTIONS] [FILES]...
```

There are no subcommands. Torrent files, magnet links and URLs given as positional arguments are loaded **started** at startup. `data:` URIs with base64-bencoded metainfo are also accepted.

### Options

| Flag | Value | Description |
|---|---|---|
| `-b` | `IP` | Bind address for the peer listener. |
| `-d`, `--directory` (alias `--download-dir`) | `DIR` | Default download directory. |
| `-w`, `--watch-dir` (alias `--watch`) | `DIR` | Watch directory: `.torrent` files are auto-loaded and started (non-recursive, live monitoring with 200 ms debounce). |
| `-i` | `IP` | Local address for outgoing connections. |
| `-p` | `PORT_RANGE` | Listen port range `first-last` (default `6881-6999`). `0-0` disables the listener. |
| `--proxy` | `URL` | Outbound peer proxy: `socks5://`, `socks5h://`, `http://`, `https://`. |
| `-s` | `DIR` | Session directory. |
| `-n` | — | Do not load any config file. Also skips TOML-only features (metrics, Web UI, log file). |
| `-c` | `FILE` | Explicit config file (`.toml` or `.rc`); overrides the [search order](Configuration-Guide#file-lookup-order). |
| `--daemon` | — | Run headless without the TUI. |
| `--grpc-port` | `PORT` | Open a gRPC TCP server. A bare port binds `127.0.0.1`. |
| `--grpc-socket` | `PATH` | Open a gRPC Unix socket (`~/` expanded). |
| `--metrics-addr` | `ADDR` | Bind address for the Prometheus `/metrics` HTTP server (e.g. `127.0.0.1:9090`). Overrides TOML `[metrics] addr`. |
| `--webui-addr` | `ADDR` | Bind address for the native Web UI (e.g. `127.0.0.1:8080`). Overrides TOML `[webui] addr`. |
| `--webui-token` | `TOKEN` | Web UI bearer token. Overrides TOML `[webui] token`. An empty token disables authentication (callers stay untrusted/read-only). |
| `-o` | `key=value` | Set an arbitrary option (repeatable). Applied after the built-in CLI mappings. |
| `-O` | `COMMAND` | Execute a raw rc command line (repeatable). Applied after `-o`. |
| `-D`, `-I`, `-K` | — | Accepted for rTorrent compatibility; no effect. |
| `-h`, `--help` | — | Print help. |
| `-V`, `--version` | — | Print version. |

### Override precedence

Config file → built-in CLI mappings (`-b`, `-i`, `-p`, `--proxy`, `-d`, `-s`, `--grpc-port`, `--grpc-socket`) → `-o` entries → `-O` entries. Later wins.

### Examples

```bash
# Interactive TUI with a watch directory
rstorrent -d ~/Downloads -w ~/Downloads/watch

# Headless daemon with Web UI, metrics and gRPC
rstorrent --daemon -c /etc/rstorrent/rstorrent.toml \
  --webui-addr 127.0.0.1:8080 --webui-token secret \
  --metrics-addr 127.0.0.1:9090 --grpc-port 50051

# One-off overrides without editing the config
rstorrent -o throttle.global_up.max_rate.set_kb=1000 \
          -O 'schedule = ratio,60,60,"stop_on_ratio=200"'

# Load magnets at startup
rstorrent 'magnet:?xt=urn:btih:...' ubuntu.torrent
```

### Interactive vs daemon mode

The TUI starts only when `--daemon` is not set and both stdin and stdout are terminals; otherwise the headless loop runs. In daemon mode the engine ticks every 50 ms.

### Signals

| Signal | Behavior |
|---|---|
| `SIGINT` (first) | Graceful, ordered shutdown. |
| `SIGINT` (second) | Immediate exit (code 130). |
| `SIGTERM` | Quick shutdown. |
| `SIGHUP` | Normal shutdown. |
| `SIGWINCH` | Terminal redraw. |

Shutdown is bounded and ordered: listeners → DHT → coordinators → disk flush → disk threads → session save → lock release. In the TUI, double `Ctrl-Q` forces exit with a 10 s checkpoint timeout.

### Session directory

Fallback chain: `-s` → configured `session.path` → `$XDG_DATA_HOME/rstorrent/session` → `~/.local/share/rstorrent/session` → `./.session`. The directory is locked; a second instance on the same session aborts at startup.

### Environment variables

| Variable | Effect |
|---|---|
| `RUST_LOG` | Per-target tracing filter (combined with TOML `[logging] level`). |
| `RSTORRENT_METRICS_FILE` | Path for the JSONL metrics recorder. |
| `XDG_CONFIG_HOME` / `XDG_DATA_HOME` / `HOME` | Config and session lookup roots. |

---

## `rstorrent-tui` (standalone client)

```
rstorrent-tui --endpoint <ENDPOINT> [--theme <THEME>] [--refresh-ms <MS>]
```

| Flag | Default | Description |
|---|---|---|
| `--endpoint` | required | Daemon endpoint: `host:port`, `tcp://host:port` or `unix:///path/to.sock`. |
| `--theme` | `default` | Colour theme name (see below). |
| `--refresh-ms` | `500` | UI refresh interval in milliseconds. |

```bash
rstorrent-tui --endpoint 127.0.0.1:50051
rstorrent-tui --endpoint unix:///tmp/rstorrent.sock --theme nord
```

### Client config file

`$XDG_CONFIG_HOME/rstorrent-tui/config.toml` (or `~/.config/rstorrent-tui/config.toml`):

```toml
endpoint = "127.0.0.1:50051"
refresh_ms = 500
keys_layout = "qwerty"      # qwerty | azerty | qwertz | dvorak
theme_name = "tokyonight"

[theme]                     # optional per-colour overrides
background = "#1a1b26"
foreground = "#c0caf5"
highlight = "#7aa2f7"
progress_bar = "#9ece6a"
error = "#f7768e"
```

CLI flags override the config file.

### Themes

Theme names are case-insensitive and ignore spaces/dashes:

| Theme | Accepted names |
|---|---|
| Tokyo Night | `tokyonight`, `tokyo`, `default` |
| Catppuccin Mocha | `catppuccinmocha`, `catppuccin`, `mocha` |
| Dracula | `dracula` |
| Nord | `nord` |
| Monokai Pro | `monokaipro`, `monokai` |
| ASCII plain | `asciiplain`, `ascii`, `plain` |
