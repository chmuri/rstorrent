# Terminal UI & Keybindings

`rstorrent` ships two terminal interfaces, both built with [Ratatui](https://github.com/ratatui/ratatui) and [Crossterm](https://github.com/crossterm-rs/crossterm):

| Surface | Binary | Engine |
| --- | --- | --- |
| **Interactive UI** | `rstorrent` (when stdin/stdout are a TTY) | Embedded in-process — no socket, no round-trips |
| **Standalone client** | `rstorrent-tui` | Connects to a daemon over [gRPC](gRPC-and-Remote-Control) |

Running `rstorrent --daemon` (or with non-TTY stdin/stdout) skips the UI entirely. Full key tables for both surfaces are on the [Keyboard Shortcuts](Keyboard-Shortcuts) page.

---

## Interactive UI (embedded)

### Views

Eight category views, switched with `1`–`8`, `h`/`l` (or arrow keys), or by clicking the tab pills / sidebar:

| Key | View | Matches |
| --- | --- | --- |
| `1` | All | everything |
| `2` | Downloading | state Downloading |
| `3` | Seeding | state Seeding |
| `4` | Active | transferring now |
| `5` | Paused | state Paused |
| `6` | Complete | fully downloaded |
| `7` | Error | state Error |
| `8` | Hashing | state Checking |

### Layout

The layout adapts to the terminal width:

- **Compact** (< 100 cols): slim header, inspector as a bottom drawer.
- **Standard** (100–139 cols): full header with rate sparklines and disk gauge, taller drawer.
- **UltraWide** (≥ 140 cols): torrent table and inspector side-by-side (60/40).

The header shows live download/upload sparklines (60-sample history), torrent counts, DHT node count, listen port, and a disk-space gauge (warning ≥ 75%, error ≥ 90%). The status bar shows the current mode (Normal / Visual / Filter / Command / Inspect), temporary messages, and context hints.

### Torrent Inspector

There is no separate details screen — details live in an inspector drawer toggled with `i`/`Enter` and focused with `Tab`. Six tabs (`1`–`6` while focused):

1. **General** — progress, ratio, rates, pieces, hash, save path, swarm counts, private flag, status message.
2. **Trackers** — tier, status, seeds/peers, next announce, URL.
3. **Peers** — address, flags, client, rates, progress.
4. **Files** — per-file priority and progress; `Space` cycles file priority.
5. **Piece Map** — adaptive piece heatmap (verified / partial / in-flight / swarm-available), in-flight block pipeline, optional braille high-resolution mode.
6. **Logs** — per-torrent event view.

### Modals

- **Add Torrent** (`a` / `Ctrl+N`): source (magnet, URL, or `.torrent` path), destination, start-immediately and sequential checkboxes. *Note: in embedded mode the destination and sequential fields are currently ignored — the torrent loads into the default directory; toggle sequential afterwards with `m`.*
- **Confirm Remove** (`x` / `Delete`): keep-data vs permanently-delete-files choice, bulk-aware, shows total disk space. Data deletion is parallelized.
- **Theme Picker** (`t` / `S`): live palette preview, applied on `Enter`.
- **Shortcuts Cheatsheet** (`?` / `F1`).
- **Command Palette** (`:` / `Ctrl+P`): fuzzy-filtered list of common actions with their shortcuts.

### Filtering and sorting

- `/` or `f` opens a live filter (matches name or info-hash as you type); `Enter` commits, `Esc` cancels.
- The list is sorted by download rate (descending); there is no user-facing sort control yet.

### Themes

Six built-in themes, picked at runtime (not persisted between runs):

| Theme | Character |
| --- | --- |
| Tokyo Night (default) | modern dark |
| Catppuccin Mocha | pastel |
| Dracula | high contrast |
| Nord | arctic oceanic |
| Monokai Pro | classic warm |
| Classic ASCII / Minimal | plain glyphs and borders, for limited terminals |

### Mouse

- Wheel scrolls the torrent list.
- Click a row to focus it; **Ctrl+click** toggles selection; **Shift+click** selects a range.
- Click view-tab pills or sidebar rows to switch views.
- Click inspector tabs/content to focus them; click `[i: Close]` to close the drawer.
- Clicking the rightmost footer cells quits.

---

## Standalone Client (`rstorrent-tui`)

Connects to a daemon over gRPC (`--endpoint host:port` or `unix:///path`); see the [CLI Reference](CLI-Reference) for flags and the TOML config file. It reproduces the classic rTorrent workflow:

- **Ten daemon views** on the number row: `1` main, `2` name, `3` started, `4` stopped, `5` complete, `6` incomplete, `7` hashing, `8` seeding, `9` leeching, `0` active.
- **Details screen** with six tabs switched by digits: Info, Files, Peers, Trackers, Chunks, Logs. The Files tab supports multi-select and priority cycling.
- **Prompts**: `Backspace` = load torrent/magnet, `Ctrl+X` = execute any RPC command (`command=args` syntax), `/`/`f`/`F` = filter (name or hash). Prompts have history (100 entries) and full line editing.
- **Context menu** (`m`, or right-click): Start/Stop/Close/Check Hash/Priority/Remove, bulk-aware when torrents are selected.
- **Command palette** (`:` / `Ctrl+P`) and **theme picker** (`t`).
- **Log overlay** (`L`): last 500 daemon log messages.
- **Throttle keys** in four keyboard layouts (qwerty/azerty/qwertz/dvorak) with 1/5/50 KiB/s steps — these set real engine rates.
- **Resilient connection**: 2 s pre-flight probe, 5 s RPC timeout, automatic reconnection with exponential backoff (1→30 s cap) and a 10 s health check; the status bar shows connection state and reconnect countdown.

Mouse: wheel scrolling, left-click opens details on the focused row, right-click opens the context menu, Ctrl+click toggles selection, tab-bar clicks switch detail tabs.

Themes: the same six palettes as the interactive UI, selected via `--theme` or `theme_name` in the config file, with optional per-color overrides under `[theme]`.

---

## Legacy rTorrent UI Compatibility

The classic rTorrent screen/keymap layer (the one driven by `ui.keymap.style` = default/emacs/vi, `keys.layout`, `ui.color.*`, `ui.input.history.size`, etc.) is kept as **RPC state**: those `ui.*` commands work over SCGI/XML-RPC/gRPC and are covered by tests, but the shipped binaries render the two modern surfaces above, not the legacy screen. The standalone client's throttle layout follows `keys_layout` from its own config.

---

## Related Pages

- [Keyboard Shortcuts](Keyboard-Shortcuts) — complete key tables for both surfaces
- [CLI Reference](CLI-Reference) — `rstorrent-tui` flags and config
- [gRPC & Remote Control](gRPC-and-Remote-Control) — daemon setup for the standalone client
