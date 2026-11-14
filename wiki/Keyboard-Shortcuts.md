# Keyboard Shortcuts

Complete key tables for the two shipped terminal surfaces. Bindings were verified against the v0.21.0 source (`rt-tui` input router/FSM and `rt-tui-client` key handler, plus their test suites). For an overview of views, layout, and themes see [Terminal UI & Keybindings](Terminal-UI-and-Keybindings).

Notation: `Ctrl+Q` = hold Ctrl and press Q (also written `^Q`); bindings are case-sensitive; `Space` = space bar.

---

## 1. Interactive UI (`rstorrent` with a TTY)

### Global

| Key | Action |
| --- | --- |
| `q` / `Ctrl+Q` | Quit gracefully (session saved). |
| `Ctrl+C` | Quit (in Visual mode: clear selection instead). |
| `:` / `Ctrl+P` | Open the command palette. |
| `?` / `F1` | Open the shortcuts cheatsheet. |
| `t` / `S` | Open the theme picker. |
| `/` / `f` | Open the live filter. |

### Views

| Key | Action |
| --- | --- |
| `1`–`8` | All / Downloading / Seeding / Active / Paused / Complete / Error / Hashing. |
| `h` / `Left` | Previous view (wraps). |
| `l` / `Right` | Next view (wraps). |

### Navigation and selection

| Key | Action |
| --- | --- |
| `j` / `Down`, `k` / `Up` | Move the cursor. |
| `g` / `Home`, `G` / `End` | Jump to first / last row. |
| `Ctrl+D` / `PgDn`, `Ctrl+U` / `PgUp` | Page down / up (10 rows). |
| `Space` | Toggle selection and advance. |
| `v` / `V` | Toggle selection in place (Visual mode). |
| `Ctrl+A` | Select all in the current view. |
| `*` | Invert selection. |
| `Esc` | Clear selection (Visual mode). |

### Torrent actions

Actions apply to the selection, or to the focused torrent when nothing is selected.

| Key | Action |
| --- | --- |
| `s` | Start. |
| `p` | Pause/stop. |
| `x` / `Delete` | Open the remove confirmation (keep data vs delete files). |
| `+` / `=` | Priority up (clamped 0–3). |
| `-` / `_` | Priority down. |
| `r` | Force hash recheck. |
| `a` / `Ctrl+N` | Open the Add Torrent modal. |
| `m` | Toggle sequential download for the focused torrent. |

### Bandwidth

| Key | Action |
| --- | --- |
| `]` / `[` | Download limit +50 / −50 KiB/s. |
| `}` / `{` | Upload limit +50 / −50 KiB/s. |
| `0` | Toggle uncapped ↔ 10 MiB/s (both directions). |

### Inspector

| Key | Action |
| --- | --- |
| `i` / `Enter` | Open/close the inspector drawer. |
| `Tab` | Focus the inspector. |

While the inspector is focused:

| Key | Action |
| --- | --- |
| `1`–`6` | General / Trackers / Peers / Files / Piece Map / Logs. |
| `h` / `Left` / `Shift+Tab`, `l` / `Right` / `Tab` | Previous / next tab. |
| `j` / `Down`, `k` / `Up` | Move the item cursor. |
| `Space` | Cycle file priority (Files tab). |
| `Esc` / `i` / `q` / `Enter` | Return focus to the list. |

### Filter mode

Live filtering on name or info-hash. `Enter` commits, `Esc` cancels, `Backspace` deletes, `Ctrl+V` / `Shift+Insert` pastes from the system clipboard, printable characters re-filter as you type.

### Command palette mode

`Up`/`Down` (or `Tab`/`Shift+Tab`) move, `Enter` executes, `Esc` closes, typing fuzzy-filters the command list, `Ctrl+V` / `Shift+Insert` pastes.

### Add Torrent modal

Six fields: source, destination, start checkbox, sequential checkbox, Cancel, Add.

| Key | Action |
| --- | --- |
| `Tab` / `Down`, `Shift+Tab` / `Up` | Next / previous field (wraps). |
| `Left` / `Right`, `Home` / `Ctrl+A`, `End` / `Ctrl+E` | Cursor movement in text fields. |
| `Space` | Types a space in text fields; toggles checkboxes/buttons otherwise. |
| `Enter` | Submit (ignored when Cancel is focused or source is empty). |
| `Esc` / `Ctrl+C` | Close. |
| `Ctrl+V` / `Shift+Insert` | Paste. |

*In embedded mode the destination and sequential fields are currently ignored; use `m` after loading to toggle sequential.*

### Remove confirmation modal

| Key | Action |
| --- | --- |
| `Up` / `k` / `s` / `r` | Choose "keep data". |
| `Down` / `j` / `d` | Choose "delete files". |
| `Tab` / `Space` | Toggle the delete-data choice. |
| `Left` / `Right` | Move between Cancel and Confirm. |
| `Enter` / `y` | Confirm (only when Confirm is focused). |
| `Esc` / `n` | Cancel. |

### Theme picker / help

Theme picker: `Up`/`k`, `Down`/`j` preview live, `Enter` applies, `Esc` cancels. Cheatsheet: `Esc` / `?` / `q` / `Enter` closes.

### Mouse

Wheel scrolls; click selects a row; Ctrl+click toggles selection; Shift+click selects a range; click view pills/sidebar to switch views; click inspector tabs/content to focus; `[i: Close]` closes the drawer; rightmost footer cells quit.

---

## 2. Standalone client (`rstorrent-tui`)

### Global

| Key | Action |
| --- | --- |
| `Ctrl+Q` | Quit. Press twice to force an immediate exit. |
| `Backspace` | Open the load prompt (torrent path / magnet / URL). |
| `Ctrl+X` | Open the command prompt (`command` or `command=arg1,arg2`). |
| `/` / `f` / `F` | Open the filter prompt (name or hash substring). |

### List navigation and views

| Key | Action |
| --- | --- |
| `1`–`9`, `0` | main / name / started / stopped / complete / incomplete / hashing / seeding / leeching / active. |
| `Up` / `Down`, `PgUp` / `PgDn`, `Home` / `End` | Navigate. |
| `Enter` / `Right` / `l` | Open details. |
| `Space` | Toggle selection. |
| `Ctrl+A` | Select all. |
| `*` | Invert selection. |
| `Esc` | Clear the filter first, else clear the selection. |
| `m` | Context menu (bulk variant when a selection exists). |
| `L` | Log overlay. |
| `t` | Theme picker. |
| `:` / `Ctrl+P` | Command palette. |

### Torrent actions

With a selection these apply to every selected torrent; otherwise to the focused one.

| Key | Action |
| --- | --- |
| `Ctrl+S` / `S` | Start. |
| `Ctrl+D` / `D` | Stop if active; remove if already stopped. |
| `Ctrl+K` / `K` | Ignore commands + stop + close. |
| `Ctrl+R` / `R` | Force full re-hash (`d.complete.set=0` then `d.check_hash`). |
| `+` | Priority cycle 0→1→2→3→0. |
| `-` | Priority cycle backwards. |
| `I` | Toggle `d.ignore_commands`. |

### Details screen

| Key | Action |
| --- | --- |
| `1`–`6` | Info / Files / Peers / Trackers / Chunks / Logs tabs. |
| `Left` / `Right` / `l` | Previous / next tab (Left on Info closes details). |
| `Esc` / `Backspace` / `h` / `Ctrl+B` | Close details. |
| `Up` / `Down`, `PgUp` / `PgDn` | Scroll Logs / move item focus. |

Files tab: `x` / `v` / `Insert` toggle file selection, `Ctrl+A` select all, `i` / `I` invert, `Space` / `*` cycle priority off→normal→high, `Esc` deselects (or closes when nothing is selected).

### Prompts (load / command / filter)

| Key | Action |
| --- | --- |
| `Enter` | Submit. |
| `Esc` / `Ctrl+G` | Cancel. |
| `Left` / `Right`, `Home` / `End` | Cursor movement. |
| `Backspace` / `Delete` | Delete characters. |
| `Up` / `Down` | History (100 entries, draft preserved). |

An empty filter submission clears the filter. Pasting a magnet/path on the list auto-opens the load prompt.

### Overlays

- **Context menu**: `Up`/`Down` move (wrap), `Enter` activates, `Esc` closes. Entries: Start/Stop (state-aware), Close, Check Hash, Priority Low/Normal/High, Remove.
- **Command palette**: `Up`/`Down`, `Enter`, `Esc`/`Ctrl+G`, typing filters.
- **Theme picker**: `Up`/`Down`, `Enter` applies, `Esc`/`Ctrl+G` closes.
- **Log overlay**: `Up`/`Down` scroll; `Space`/`Esc`/`L`/`l` close.

### Throttle keys

Lowercase adjusts **upload**, uppercase adjusts **download**; the first key of each pair increases, the second decreases. Steps: small 1, medium 5, large 50 KiB/s. These set the real global rates (`throttle.global_up/down.max_rate.set`).

| Layout | Upload +/− small | Download +/− small | Upload +/− med | Download +/− med | Upload +/− large | Download +/− large |
| --- | --- | --- | --- | --- | --- | --- |
| `qwerty` (default) | `a` / `z` | `A` / `Z` | `s` / `x` | `S` / `X` | `d` / `c` | `D` / `C` |
| `azerty` | `q` / `w` | `Q` / `W` | `s` / `x` | `S` / `X` | `d` / `c` | `D` / `C` |
| `qwertz` | `a` / `y` | `A` / `Y` | `s` / `x` | `S` / `X` | `d` / `c` | `D` / `C` |
| `dvorak` | `a` / `;` | `A` / `:` | `o` / `q` | `O` / `Q` | `e` / `j` | `E` / `J` |

Select the layout with `keys_layout` in `~/.config/rstorrent-tui/config.toml`. Adjustments saturate at zero.

### Mouse

Wheel scrolls lists/overlays; left-click on the already-focused row opens details; right-click opens the context menu; Ctrl+click toggles selection; clicking the details tab bar switches tabs; menu entries activate on click.

---

## 3. Legacy rTorrent keymap (RPC compatibility)

The classic rTorrent keymap styles (`ui.keymap.style` = `default` / `emacs` / `vi`) and the `ui.*` RPC variables (colors, history size, throttle steps, list layout) are maintained as RPC state for compatibility and are exercised by the test suite, but the shipped binaries render the two surfaces above — the legacy screen itself is not drawn. Use the `ui.*` commands over SCGI/gRPC if your tooling depends on them; see the [Configuration Guide](Configuration-Guide).

---

## Known limitations

- Interactive UI: the Add Torrent modal's destination and sequential fields are ignored in embedded mode (default directory is used; `m` toggles sequential afterwards).
- Interactive UI: sorting is fixed to download rate, descending; there is no sort control yet.
- Interactive UI: some header statistics (session totals, DHT node count) are not yet sampled from the engine, and the inspector Logs tab shows static metadata rather than live logs.
- Standalone client: removal never deletes data files (`delete_data` is not sent); use RPC commands for data deletion.

If a shortcut does not match this page, please include the rstorrent version and the focused view in the issue report.
