# TDD Evidence — TASK-16 — Configuration, Theming & Log View

> **Source plan:** [`docs/tui-implementation.plan.md`](../tui-implementation.plan.md) — *TASK-16*
> **Depends on:** TASK-06 (`425eb83a`)
> **Date:** 2026-10-29

## 1. Acceptance

| # | Criterion | Status |
|---|-----------|--------|
| 1 | Config at `~/.config/rstorrent-tui/config.toml`, XDG aware | PASS (`config_path`) |
| 2 | Configurable: endpoint, refresh_ms, theme colours, keys.layout | PASS (endpoint: CLI wins, see §4) |
| 3 | Theme: background, foreground, highlight, progress_bar, error | PASS |
| 4 | `LogWidget` renders timestamped messages with scrollback | PASS |
| 5 | `l` toggles the log view; Space returns to the list | PASS (`L`, see §4) |
| 6 | Log captures connection events, command results, errors | PASS |

## 2. Gates

| Gate | Command | Result |
|------|---------|--------|
| RED | `command cargo test -p rt-tui-client --test test_config` | FAIL — `unresolved import rt_tui_client::config` |
| GREEN | same | PASS — 11 tests |
| Lint/format/check | workspace clippy `-D warnings`, `fmt --check`, `check --all-targets` | PASS |
| Coverage | `llvm-cov -p rt-tui-client` | 86.97 % total, `config.rs` 85.14 %, `log.rs` 88.89 % |

## 3. Tests

| # | Guarantee | Test | Result |
|---|-----------|------|--------|
| 1 | Defaults: 500 ms, qwerty | `default_config_is_valid` | PASS |
| 2 | TOML parses endpoint/refresh/layout | `config_parses_from_toml` | PASS |
| 3 | Log widget renders messages | `log_widget_renders_messages` | PASS |
| 4 | Config path honours `XDG_CONFIG_HOME` | `config_path_follows_xdg` | PASS |
| 5 | Colours parse from names and `#rrggbb` | `theme_colors_parse_names_and_hex` | PASS |
| 6 | Theme block feeds resolved colours | `config_theme_feeds_the_app_theme` | PASS |
| 7 | Log is bounded at 500 and keeps the newest | `log_state_is_bounded_and_keeps_the_newest` | PASS |
| 8 | Entries carry `HH:MM:SS` | `log_entries_carry_a_timestamp` | PASS |
| 9 | `L` opens the log, Space returns | `log_view_toggles_with_l_and_returns_with_space` | PASS |
| 10 | The frame renders the log body | `draw_renders_the_log_view_when_toggled` | PASS |
| 11 | `App::log` records messages | `commands_are_logged` | PASS |

## 4. Deviations and gaps

- **`L` (shift+L) toggles the log, not `l`.** `l` is already the vi-style "open
  details" binding shipped in TASK-07 and covered by tests; reusing it would have
  silently changed that contract. Documented instead of dropped.
- **Endpoint precedence: CLI wins.** `--endpoint` stays required (TASK-01
  contract), so the config's `endpoint` is only reported when it conflicts. The
  refresh interval and the throttle layout *do* come from the config when the CLI
  uses its default.
- **Timestamps are UTC** (`HH:MM:SS` from the Unix epoch), because no date/time
  crate is a dependency and local-offset handling would need one.
- **Gap:** the theme's `background`/`foreground` are stored but only
  `highlight`/`progress_bar`/`error` are consumed by widgets so far.
- **Gap:** `AppState.filter` from the `/` prompt is still not applied to the list.

## 5. Changed files

`src/config.rs` (new), `src/widgets/log.rs` (new), `src/state.rs`
(`LogEntry`/`LogState`, `show_log`, `theme`), `src/app.rs` (`with_theme`, `log`,
log view routing/drawing), `src/widgets/status_bar.rs` (theme colour),
`src/main.rs` (config load), `Cargo.toml` (serde, toml), `tests/test_config.rs`.
