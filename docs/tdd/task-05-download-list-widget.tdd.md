# TDD Evidence — TASK-05 — Download List Widget

> **Source plan:** [`docs/tui-implementation.plan.md`](../tui-implementation.plan.md) — section *TASK-05 — Download List Widget*
> **Skill:** `skills/tdd-workflow` (RED → GREEN → REFACTOR)
> **Rules applied:** `rules/rust/coding-style.md`, `rules/rust/patterns.md`
> **Depends on:** TASK-03 (`7aaaf3bd`), TASK-04 (`f1572a76`)
> **Date:** 2026-10-27

## 1. User journey (from plan)

> As a user, I see the torrent list with: name, size, progress bar, down/up rate,
> state, ratio. I navigate with Up/Down/PgUp/PgDn/Home/End and the focused row is
> highlighted.

| # | Acceptance criterion | Status |
|---|----------------------|--------|
| 1 | `DownloadListWidget` implements `ratatui::Widget` | PASS |
| 2 | Columns: name, size, done %, progress bar, ↓ rate, ↑ rate, state, ratio | PASS |
| 3 | Focus row highlighted with `Style::new().reversed()` | PASS |
| 4 | Scrolling viewport with PgUp/PgDn, Home/End | PASS |
| 5 | Empty list shows "No torrents" centered | PASS |
| 6 | Column widths adapt, minimum viable at 60 cols | PASS |

## 2. Validation commands (actually run)

Remote builder still unreachable → local escape hatch (`command cargo ...`).

| Gate | Command | Result |
|------|---------|--------|
| RED | `command cargo test -p rt-tui-client --test test_download_list_widget` | FAIL (expected) |
| GREEN | `command cargo test -p rt-tui-client` | PASS — 84 tests (12 widget) |
| Lint | `command cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| Format | `command cargo fmt --check` | PASS |
| Compile gate | `command cargo check --workspace --all-targets` | PASS |
| Coverage | `command cargo llvm-cov -p rt-tui-client --summary-only --ignore-filename-regex 'main\.rs'` | 86.80 % lines |
| Manual E2E | pty 100×20 against a mock daemon | PASS (see §5) |

## 3. RED evidence

```text
error[E0433]: cannot find `widgets` in `rt_tui_client`
  --> crates/rt-tui-client/tests/test_download_list_widget.rs:12:20

error[E0432]: unresolved import `rt_tui_client::widgets`
  --> crates/rt-tui-client/tests/test_download_list_widget.rs:13:20

error[E0609]: no field `ratio` on type `TorrentViewModel`
  --> crates/rt-tui-client/tests/test_app_state.rs:125:17
```

Committed as RED checkpoint `f6b29e93`.

## 4. GREEN evidence

```text
Running tests/test_app_state.rs            test result: ok. 21 passed
Running tests/test_cli.rs                  test result: ok. 5 passed
Running tests/test_download_list_widget.rs test result: ok. 12 passed
Running tests/test_endpoint.rs             test result: ok. 13 passed
Running tests/test_event_loop.rs           test result: ok. 15 passed
Running tests/test_grpc_bridge.rs          test result: ok. 15 passed
Running tests/test_run.rs                  test result: ok. 3 passed
```

Committed as `a63156e3`.

The first GREEN attempt failed on `renders_torrent_name_and_state`: the initial
column widths (10-cell bar, 10-cell rates, 10-cell state) left only 14 cells for
the name at 80 columns, truncating `ubuntu-26.04.iso`. Widths were tightened
(bar 8, rates 9, state 11) so the name keeps 17 cells at 80 columns — the plan's
own contract caught a real layout bug, which is exactly what it is for.

## 5. Manual end-to-end render

A throwaway mock daemon (3 torrents) plus a pty at 100×20:

```text
┌rstorrent-tui───────────────────────────────────────────────────────────────┐
│Name                Size     Done Progress  ↓ Rate    ↑ Rate  State    Ratio │
│ubuntu-26.04.iso    2.0 GB    50% ████░░░░  1.4 MB  146.5 KB  downloading 2.000
│debian-13.iso       1.0 GB   100% ████████  0.0 KB    0.0 KB  seeding     2.000
│arch.iso          768.0 MB    12% █░░░░░░░ 20.0 KB    2.0 KB  downloading 2.000
│connected · 3 torrents                                                        │
└──────────────────────────────────────────────────────────────────────────────┘
COMMAND_EXIT_CODE="0"
```

Focus row 0 was inverted (verified by the REVERSED cell assertion, not visible in
the escape-stripped capture). The helper file was deleted afterwards — it is not
part of the deliverable.

## 6. Test specification

| # | What is guaranteed | Test | Type | Result |
|---|--------------------|------|------|--------|
| 1 | An empty list renders "No torrents" | `test_download_list_widget.rs:renders_empty_list_message` | unit (`Buffer`) | PASS |
| 2 | Name and state columns render their values | `renders_torrent_name_and_state` | unit | PASS |
| 3 | The focused row carries `Modifier::REVERSED` | `focus_row_is_highlighted` | unit | PASS |
| 4 | All eight columns fit at 120 columns | `all_columns_fit_a_wide_terminal` | unit | PASS |
| 5 | Bar and ratio disappear below 80 columns | `bar_and_ratio_are_dropped_below_eighty_columns` | unit | PASS |
| 6 | Below 40 columns only name and state remain | `very_narrow_terminals_keep_only_name_and_state` | unit | PASS |
| 7 | A 40-column render still shows the name | `narrow_render_still_shows_the_name` | unit | PASS |
| 8 | The header carries the column labels | `renders_header_labels` | unit | PASS |
| 9 | Progress bar and ratio render as text | `renders_progress_bar_and_ratio` | unit | PASS |
| 10 | Scrolling keeps the focused row visible and scrolls the first row away | `viewport_scrolls_to_keep_the_focus_visible` | unit | PASS |
| 11 | PgDn/PgUp move by one viewport | `page_down_and_page_up_move_by_one_viewport` | unit (App) | PASS |
| 12 | Arrow keys move one row; Home/End jump to the bounds | `arrow_keys_move_one_row_and_home_end_jump` | unit (App) | PASS |
| 13 | Ratio is `uploaded / downloaded` with three decimals | `test_app_state.rs:torrent_view_model_computes_share_ratio` | unit | PASS |
| 14 | Ratio is `0.000` when nothing was downloaded | `torrent_view_model_ratio_is_zero_without_downloaded_bytes` | unit | PASS |

## 7. Coverage and known gaps

```text
Filename                    Regions  Missed  Cover   Lines  Missed  Cover
app.rs                          215      89  58.60%     133      50  62.41%
widgets/download_list.rs        210       1  99.52%     127       1  99.21%
TOTAL                           973     145  85.10%     674      89  86.80%
```

Known gaps:

- **`app.rs` stays at 62 %** for the same reason as TASK-04: `run`/`read_events`/
  `poll_input` need a TTY. The new key bindings are covered.
- **The focused row style is asserted, not observed visually.** The pty capture
  strips SGR attributes; the `buf.cell((0, 2)).modifier` assertion is the
  evidence.
- **No mouse/wheel scrolling** — not in the plan; TASK-10/13 may add it.
- **`viewport()` is stateless** (centres the focus), so the view can shift when
  the focus moves back and forth; a sticky offset would need state in `AppState`.
  Deliberate: no state to keep in sync with refresh.
- Only the affected crate is tested locally; full `cargo test --workspace` stays
  a CI/builder gate.

## 8. REFACTOR notes

1. **Widths tightened after the RED contract exposed truncation** — the name
   column is now the only `Min` constraint and gets the leftover space; every
   other column is a fixed `Length`, so the table degrades predictably.
2. **`cell_for` borrows the view model** (`Cell<'_>` from `&str` fields) instead
   of cloning strings per row per frame; only the done-% cell allocates, because
   it is a formatted number.
3. **`Status` line kept**: TASK-04's `draw` tests assert the title and connection
   status, so the placeholder chrome stayed and the list was placed inside it.
   TASK-06 replaces both with real title/status bars.
4. **`Column` is an enum, not a struct of booleans**, so the header, the width
   request and the cell builder all match on the same value — adding a column is
   one place, and every `match` is exhaustive.
5. Rules review: no `unwrap()`/`expect()` in `src/`, no per-frame `String` clones
   for static fields, `viewport`/`columns_for_width` are pure and testable.

## 9. Design decisions worth reviewing

- **Row highlight via `Row::style(reversed())`.** `Table` patches the row style
  over the whole row area, so every cell of the focused line is inverted without
  styling cells one by one.
- **Adaptive columns by width thresholds** (80/60/40) rather than percentage
  constraints: predictable at any width and trivially testable.
- **Page size = visible list rows** (`terminal_rows - 1` header), so PgDn moves
  exactly one screen of torrents.
- **Ratio format `{:>6.3}`** matches rTorrent's three-decimal display and fits
  the column.
- **Empty state replaces the table entirely** (no header), so the message is
  centred in the screen rather than pushed below an empty header.

## 10. Changed files

| Action | Path | Notes |
|--------|------|-------|
| CREATE | `crates/rt-tui-client/src/widgets/mod.rs` | module wiring |
| CREATE | `crates/rt-tui-client/src/widgets/download_list.rs` | `DownloadListWidget`, `Column`, `columns_for_width`, `viewport` (221 lines) |
| CREATE | `crates/rt-tui-client/tests/test_download_list_widget.rs` | 12 tests (220 lines) |
| MODIFY | `crates/rt-tui-client/src/state.rs` | `ratio`/`ratio_display`, `focus_page_up/down` |
| MODIFY | `crates/rt-tui-client/src/app.rs` | list rendering, navigation keys, `page_size` |
| MODIFY | `crates/rt-tui-client/src/lib.rs` | `pub mod widgets` + re-export |
| MODIFY | `crates/rt-tui-client/tests/test_app_state.rs` | 2 ratio tests |

Commits:

| Commit | Stage |
|--------|-------|
| `f6b29e93` | `test:` RED checkpoint |
| `a63156e3` | `feat:` GREEN + refactor |

## 11. Not in scope (next plan task)

`TASK-06 — Title Bar & Status Bar Widgets` replaces the current block title and
status line with the real rTorrent-style bars (session totals, throttle, port).
