# Terminal UI & Keybindings

`rstorrent` includes an interactive Terminal User Interface built with [Ratatui](https://github.com/ratatui/ratatui) and [Crossterm](https://github.com/crossterm-rs/crossterm). It offers mouse and keyboard navigation, multi-torrent selection, dynamic rate throttles, and an inspector drawer.

---

## Global Navigation

| Key | Action |
|---|---|
| `q` / `Ctrl+Q` | Quit rtorrent cleanly (graceful shutdown with session checkpoint). |
| `Ctrl+C` | Interrupt / shutdown. |
| `?` or `F1` | Open interactive keyboard cheatsheet overlay. |
| `:` | Open command palette. |
| `t` | Cycle or open color theme picker (Tokyo Night, Catppuccin, Gruvbox, Nord, Solarized, Monokai, Dracula). |

---

## Torrent List & Category Views

| Key | Action |
|---|---|
| `Up` / `Down` or `k` / `j` | Move selection cursor up / down. |
| `Left` / `Right` or `h` / `l` | Switch between category views (`All`, `Downloading`, `Seeding`, `Active`, etc.). |
| `1` .. `8` | Direct jump to category view tab. |
| `Space` | Toggle multi-selection on focused torrent. |
| `Ctrl+A` | Select all visible torrents in current view. |
| `*` | Invert selection. |
| `s` | Start selected torrents. |
| `p` | Pause selected torrents. |
| `x` | Open safe removal confirmation dialog (optional payload file deletion). |
| `+` / `-` | Increase / decrease torrent priority. |
| `Enter` or `i` | Open and focus the Torrent Inspector drawer for the highlighted item. |

---

## Bandwidth & Throttle Hotkeys

| Key | Action |
|---|---|
| `a` / `z` | Increase / decrease upload throttle limit (+/- 10 KB/s, Shift/Ctrl for 100/1000 KB/s). |
| `A` / `Z` | Increase / decrease download throttle limit (+/- 10 KB/s, Shift/Ctrl for 100/1000 KB/s). |

---

## Torrent Inspector Drawer

When the drawer is open (`Enter` or `i`):

- **Tabs**:
  - `1:General`: Overview of completed chunks, ETA, save path, hash, and status messages.
  - `2:Trackers`: List of tracker tiers, seeds/peers, last announce status, and next scrape timer.
  - `3:Peers`: Live swarm peers, download/upload rates, client version, and encryption status.
  - `4:Files`: Hierarchical file tree with completion percentages; press `Space` to cycle priorities (`Off`, `Normal`, `High`).
  - `5:Piece Map`: Braille-density piece heatmap showing verified and in-progress chunks.
  - `6:Logs`: Diagnostic event log for this torrent.
- **Navigation inside drawer**:
  - `1` .. `6`: Directly switch to inspector tab.
  - `Tab` / `Shift+Tab`: Cycle between inspector tabs.
  - `+` / `-`: Change file priority in the Files tab.
  - `Esc` or `i`: Close drawer and return focus to the torrent list.

---

## Mouse Support

- Click directly on category view tabs (`[1] All`, `[2] Downloading`, etc.) to switch filters.
- Click any torrent row in the table to select it; double click to open the inspector drawer.
- Click any inspector tab (`[1:General]`, `[★ 5:Piece Map]`, etc.) to switch inspector views.
- Click `[i: Close]` in the drawer header to close it.
- Scroll wheel scrolls torrent lists, file lists, and log views.
