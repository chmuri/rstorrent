# TDD Evidence — TASK-06 — Title Bar & Status Bar Widgets

> **Source plan:** [`docs/tui-implementation.plan.md`](../tui-implementation.plan.md) — section *TASK-06 — Title Bar & Status Bar Widgets*
> **Skill:** `skills/tdd-workflow` (RED → GREEN → REFACTOR)
> **Rules applied:** `rules/rust/coding-style.md`, `rules/rust/patterns.md`
> **Depends on:** TASK-03 (`7aaaf3bd`), TASK-05 (`a63156e3`)
> **Date:** 2026-10-29

## 1. User journey (from plan)

> As a user, I see the top bar with "rstorrent v0.16.1 — [main] 5 torrents" and
> the bottom bar with throttle up/down rates, listen port, and connection status.

| # | Acceptance criterion | Status |
|---|----------------------|--------|
| 1 | `TitleBarWidget` renders client version, active view, torrent count | PASS |
| 2 | `StatusBarWidget` renders ↑/↓ global rate, ↑/↓ throttle limits, listen port, connection indicator | PASS |
| 3 | Connection status: connected / disconnected / reconnecting | PASS (coloured `●` + label; emoji avoided — see §9) |
| 4 | Widgets adapt to narrow terminals (hide less important fields first) | PASS |

## 2. Validation commands (actually run)

Remote builder still unreachable → local escape hatch (`command cargo ...`).

| Gate | Command | Result |
|------|---------|--------|
| RED | `command cargo test -p rt-tui-client --test test_bars` | FAIL (expected) |
| GREEN | `command cargo test -p rt-tui-client` | PASS — 96 tests (12 bar) |
| Lint | `command cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| Format | `command cargo fmt --check` | PASS |
| Compile gate | `command cargo check --workspace --all-targets` | PASS |
| Coverage | `command cargo llvm-cov -p rt-tui-client --summary-only --ignore-filename-regex 'main\.rs'` | 88.21 % lines |
| Manual E2E | pty at 90×14 and 55×14 against a mock daemon | PASS (see §5) |

## 3. RED evidence

```text
error[E0432]: unresolved imports `rt_tui_client::widgets::StatusBarWidget`,
              `rt_tui_client::widgets::TitleBarWidget`
  --> crates/rt-tui-client/tests/test_bars.rs:9:30

error[E0560]: struct `SessionStats` has no field named `throttle_up_display`
  --> crates/rt-tui-client/tests/test_bars.rs:127:9

error[E0599]: no variant, associated function, or constant named `Reconnecting`
              found for enum `ConnectionStatus`
  --> crates/rt-tui-client/tests/test_bars.rs:177:42
```

Committed as RED checkpoint `583868cf`.

**Plan deviation (recorded, not silently widened):** the plan's snippet writes
`state.active_view = "main".to_string()` and `state.connection_status`, but
TASK-03 shipped `active_view: ActiveView` (enum) and `connection`. The contract
uses the real API; renaming shipped fields to match pseudocode would be a
regression in type safety.

## 4. GREEN evidence

```text
Running tests/test_app_state.rs            test result: ok. 21 passed
Running tests/test_bars.rs                 test result: ok. 12 passed
Running tests/test_cli.rs                  test result: ok. 5 passed
Running tests/test_download_list_widget.rs test result: ok. 12 passed
Running tests/test_endpoint.rs             test result: ok. 13 passed
Running tests/test_event_loop.rs           test result: ok. 15 passed
Running tests/test_grpc_bridge.rs          test result: ok. 15 passed
Running tests/test_run.rs                  test result: ok. 3 passed
```

Committed as `425eb83a`.

The first GREEN attempt failed one test for a test-side bug: the medium-width
case used `AppState::new()`, whose default `listen_port` is `0`, while asserting
`:6881`. The test now sets the port explicitly — the widget was correct.

## 5. Manual end-to-end render

Mock daemon (2 torrents, up 150 kB/s, down 1.5 MB/s, throttle up unlimited,
throttle down 500 kB/s, port 6881) under a pty:

```text
90×14
rstorrent v0.16.1 — [main] 2 torrents
Name              Size     Done Progress  ↓ Rate   ↑ Rate  State        Ratio
ubuntu-26.04.iso  2.0 GB    50% ████░░░░  1.4 MB  146.5 KB downloading  2.000
debian-13.iso     1.0 GB   100% ████████  0.0 KB    0.0 KB seeding      2.000
↑ 146.5 KB ↓ 1.4 MB  thr ↑ ∞ ↓ 488.3 KB  :6881  ● connected

55×14
rstorrent v0.16.1 — [main] 2 torrents
Name                Size     State
ubuntu-26.04.iso    2.0 GB   downloading
debian-13.iso       1.0 GB   seeding
↑ 146.5 KB ↓ 1.4 MB  :6881  ● connected
```

Exit code 0 on `^Q` in both runs. The helper example was deleted afterwards.

## 6. Test specification

| # | What is guaranteed | Test | Type | Result |
|---|--------------------|------|------|--------|
| 1 | Title bar shows the view name and torrent count | `test_bars.rs:title_bar_shows_view_name_and_count` | unit | PASS |
| 2 | Status bar shows the connected indicator | `status_bar_shows_connection_indicator` | unit | PASS |
| 3 | Title bar shows the client version | `title_bar_shows_client_version` | unit | PASS |
| 4 | One torrent uses the singular form | `title_bar_uses_singular_for_one_torrent` | unit | PASS |
| 5 | The version disappears below 50 columns | `title_bar_drops_the_version_on_narrow_terminals` | unit | PASS |
| 6 | Only `[view]` survives at 15 columns | `title_bar_keeps_only_the_view_when_very_narrow` | unit | PASS |
| 7 | Rates, throttles and port all render at 100 columns | `status_bar_shows_rates_port_and_throttles_when_wide` | unit | PASS |
| 8 | Throttles disappear at 55 columns, port stays | `status_bar_hides_throttles_on_medium_terminals` | unit | PASS |
| 9 | At 40 columns only rates + indicator remain | `status_bar_keeps_only_rates_and_indicator_when_narrow` | unit | PASS |
| 10 | At 12 columns only the dot remains | `status_bar_degrades_to_the_bare_indicator` | unit | PASS |
| 11 | Reconnecting and disconnected states are labelled | `status_bar_reports_reconnecting_and_disconnected_states` | unit | PASS |
| 12 | Unlimited throttles render as `∞` | `status_bar_marks_unlimited_throttle` | unit | PASS |
| 13 | The composed frame shows title bar, view and indicator | `test_event_loop.rs:draw_renders_title_bar_and_status_bar` | unit (`TestBackend`) | PASS |
| 14 | Throttle mapping yields `∞` for 0 and a rate otherwise | `test_app_state.rs:session_stats_maps_from_proto` | unit | PASS |

## 7. Coverage and known gaps

```text
Filename                    Regions  Missed  Cover   Lines  Missed  Cover
widgets/status_bar.rs            73       0 100.00%      54       0 100.00%
widgets/title_bar.rs             41       0 100.00%      23       0 100.00%
TOTAL                          1086     145  86.65%     755      89  88.21%
```

Known gaps:

- **Narrow-width thresholds are constants**, not derived from the rendered text
  length (36/50/72/20 columns). The presence/absence tests pin the behaviour, but
  changing a display format can silently make a segment clip instead of being
  dropped — a follow-up could compute the budget from the segment lengths.
- **The disconnected reason is not shown** in the bar (only the red label); it
  stays in `ConnectionStatus::Disconnected { reason }` for TASK-15 to surface.
- **`Reconnecting` is not yet produced by any code path** — TASK-15 owns the
  retry loop; the indicator already renders it.
- **`app.rs` remains ~62 %** (`run`/`read_events` need a TTY), unchanged.
- Only the affected crate is tested locally; full `cargo test --workspace` stays
  a CI/builder gate.

## 8. REFACTOR notes

1. **Formatting moved into the view model.** `SessionStats` gained
   `throttle_up_display`/`throttle_down_display` instead of the widget calling a
   formatter, keeping the TASK-03 invariant "widgets never format numbers".
2. **`SessionStats::default()` is now derived from an empty proto response**
   (`from_proto(&SessionStatsResponse::default())`) rather than `#[derive(Default)]`,
   so a fresh state renders `0.0 KB` / `∞` instead of empty cells.
3. **Placeholder chrome removed.** TASK-04's bordered block and status paragraph
   are gone; `App::draw` is a three-band `Layout` (1 / min 1 / 1). The two
   TASK-04 draw tests were retargeted at the real bars rather than deleted.
4. **`push_segment` helper** keeps the separator logic in one place; the dot is
   always rendered, so a 1-column terminal still shows state.
5. Rules review: no `unwrap()`/`expect()` in `src/`, exhaustive `match` on
   `ConnectionStatus`, no per-frame clones of owned state (only `format!` for
   composed segments).

## 9. Design decisions worth reviewing

- **`●` instead of 🟢/🔴/🟡.** Emoji are double-width in many terminals and
  would break the one-line bar arithmetic; colour conveys the same information
  and the plan's own test accepts `●`.
- **Version comes from `CARGO_PKG_VERSION`** (client version, as the plan says),
  not from the daemon's `GetVersion`, which is available for TASK-15 to display.
- **Title bar is bold, status bar is plain** — colour is reserved for the
  connection state so the eye goes there first.
- **Priority order in the status bar** is rates → throttles → port, matching the
  plan's ordering, so a 55-column terminal keeps the port but drops throttles.
- **`Reconnecting` added now** even though nothing sets it yet, because the
  acceptance criterion names three indicator states.

## 10. Changed files

| Action | Path | Notes |
|--------|------|-------|
| CREATE | `crates/rt-tui-client/src/widgets/title_bar.rs` | `TitleBarWidget` (64 lines) |
| CREATE | `crates/rt-tui-client/src/widgets/status_bar.rs` | `StatusBarWidget` (98 lines) |
| CREATE | `crates/rt-tui-client/tests/test_bars.rs` | 12 tests (206 lines) |
| MODIFY | `crates/rt-tui-client/src/state.rs` | `Reconnecting`, throttle displays, manual `Default` |
| MODIFY | `crates/rt-tui-client/src/app.rs` | three-band layout, placeholder removed |
| MODIFY | `crates/rt-tui-client/src/widgets/mod.rs`, `src/lib.rs` | module wiring + re-exports |
| MODIFY | `crates/rt-tui-client/tests/test_event_loop.rs` | draw assertions target the bars |
| MODIFY | `crates/rt-tui-client/tests/test_app_state.rs` | throttle display assertions |

Commits:

| Commit | Stage |
|--------|-------|
| `583868cf` | `test:` RED checkpoint |
| `425eb83a` | `feat:` GREEN + refactor |

## 11. Not in scope (next plan task)

`TASK-07 — Torrent Details View` adds the per-torrent detail pane backed by
`GrpcBridge::get_torrent`.
