# TDD Evidence — TASK-03 — Application State (View Model)

> **Source plan:** [`docs/tui-implementation.plan.md`](../tui-implementation.plan.md) — section *TASK-03 — Application State (View Model)*
> **Skill:** `skills/tdd-workflow` (RED → GREEN → REFACTOR)
> **Rules applied:** `rules/rust/coding-style.md`, `rules/rust/patterns.md`
> **Depends on:** TASK-02 (merged as `de890203`)
> **Date:** 2026-10-27

## 1. User journey (from plan)

> As the rendering layer, I read `AppState.torrents` and `AppState.session_stats`
> to draw the UI. As the polling loop, I call `AppState::refresh(&bridge)` and the
> state updates atomically.

| # | Acceptance criterion | Status |
|---|----------------------|--------|
| 1 | `AppState` holds `Vec<TorrentViewModel>`, `SessionStats`, `ConnectionStatus`, `ActiveView`, `FocusIndex` | PASS |
| 2 | `TorrentViewModel` maps from proto `TorrentSummary` with formatted rates/sizes | PASS |
| 3 | `SessionStats` maps from proto `SessionStatsResponse` | PASS |
| 4 | `refresh()` fetches from the bridge; errors transition to `ConnectionStatus::Disconnected` | PASS |
| 5 | State is not behind `Arc`/`Mutex` — owned by the single-threaded TUI loop | PASS |

## 2. Validation commands (actually run)

Remote builder still unreachable → local escape hatch (`command cargo ...`).

| Gate | Command | Result |
|------|---------|--------|
| RED | `command cargo test -p rt-tui-client --test test_app_state` | FAIL (expected) |
| GREEN | `command cargo test -p rt-tui-client` | PASS — 55 tests (19 app-state) |
| Lint | `command cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| Format | `command cargo fmt --check` | PASS |
| Compile gate | `command cargo check --workspace --all-targets` | PASS |
| Coverage | `command cargo llvm-cov -p rt-tui-client --summary-only --ignore-filename-regex 'main\.rs'` | 97.76 % lines |

## 3. RED evidence

```text
error[E0432]: unresolved import `rt_tui_client::state`
  --> crates/rt-tui-client/tests/test_app_state.rs:10:20
   |
10 | use rt_tui_client::state::{
   |                    ^^^^^ could not find `state` in `rt_tui_client`
```

`test_grpc_bridge.rs` compiled against the extracted mock in the same run, so the
only failure was the missing module. Committed as RED checkpoint `3d94e0e8`.

## 4. GREEN evidence

```text
Running tests/test_app_state.rs    test result: ok. 19 passed
Running tests/test_cli.rs          test result: ok. 5 passed
Running tests/test_endpoint.rs     test result: ok. 13 passed
Running tests/test_grpc_bridge.rs  test result: ok. 15 passed
Running tests/test_run.rs          test result: ok. 3 passed
```

Committed as `7aaaf3bd` (`feat: add application state view model`).

## 5. Test specification

| # | What is guaranteed | Test | Type | Result |
|---|--------------------|------|------|--------|
| 1 | A zero rate renders as `"   0.0 KB"` (six-column field) | `test_app_state.rs:torrent_view_model_formats_rate_zero_as_dash` | unit | PASS |
| 2 | 1 500 000 B/s renders as `1.4 MB` (binary units) | `torrent_view_model_formats_megabyte_rate` | unit | PASS |
| 3 | Focus stops at the last row instead of overflowing | `app_state_focus_clamps_to_bounds` | unit | PASS |
| 4 | `focus_home()` selects row 0 | `app_state_focus_home_goes_to_zero` | unit | PASS |
| 5 | 2 GiB renders as `2.0 GB` | `torrent_view_model_formats_size_with_binary_unit` | unit | PASS |
| 6 | 1.5 TB renders as `1.5 TB` (unit ladder top) | `torrent_view_model_formats_terabyte_sizes` | unit | PASS |
| 7 | 1.5 KiB renders as `1.5 KB` (unit ladder bottom) | `torrent_view_model_formats_sub_megabyte_sizes` | unit | PASS |
| 8 | Progress is `completed / size` in percent | `torrent_view_model_computes_progress_percent` | unit | PASS |
| 9 | Unknown size yields 0 % instead of a division by zero | `torrent_view_model_progress_is_zero_when_size_unknown` | unit | PASS |
| 10 | Identity, name and state name survive the mapping | `torrent_view_model_copies_identity_and_state` | unit | PASS |
| 11 | `SessionStatsResponse` maps every field incl. formatted decimals | `session_stats_maps_from_proto` | unit | PASS |
| 12 | A new state is empty, focused at 0, on `main`, disconnected | `app_state_starts_disconnected_on_main_view` | unit | PASS |
| 13 | All six `ActiveView` values map to their rTorrent RPC names | `active_view_maps_to_rpc_view_names` | unit | PASS |
| 14 | `focus_up()` saturates at 0 and `focus_end()` selects the last row | `focus_up_saturates_and_focus_end_selects_last` | unit | PASS |
| 15 | `focus_down()` on an empty list keeps index 0 and `selected()` is `None` | `focus_down_on_empty_list_keeps_zero` | unit | PASS |
| 16 | `is_connected()` is true only for `Connected` | `connection_status_reports_connected_only_when_connected` | unit | PASS |
| 17 | A successful refresh fills torrents + stats and reports `Connected` | `refresh_populates_torrents_and_stats` | integration (in-process daemon) | PASS |
| 18 | A server error flips the state to `Disconnected` carrying the cause | `refresh_marks_state_disconnected_on_server_error` | integration | PASS |
| 19 | A failed refresh does not wipe the previously rendered list | `refresh_keeps_previous_data_when_the_daemon_fails` | integration | PASS |

## 6. Coverage and known gaps

```text
Filename       Regions  Missed  Cover   Functions  Missed  Executed   Lines  Missed  Cover
bridge.rs          152       6  96.05%         19       1   94.74%     108       2  98.15%
endpoint.rs         94       4  95.74%          9       0  100.00%      60       2  96.67%
run.rs              27       0 100.00%          3       0  100.00%      24       0 100.00%
state.rs           154       4  97.40%         19       1   94.74%     120       3  97.50%
TOTAL              427      14  96.72%         50       2   96.00%     312       7  97.76%
```

Known gaps:

- **`refresh` is not exercised for the "list succeeded, stats failed" combination.**
  The current implementation propagates the stats error and keeps the old data,
  which is the same code path as the tested failure case, but the mixed outcome
  is not asserted separately.
- **`https://` endpoint branch and `src/main.rs`** remain uncovered (as in TASK-02).
- **No `focus_next_view()` / view cycling yet** — that is TASK-10 (keyboard
  shortcuts). `ActiveView` already exposes `as_str()` for the RPC argument.
- **Raw totals are not kept** in `SessionStats` (only formatted strings); the
  status bar in TASK-06 does not need them, and adding fields later is cheap.
- Only the affected crate is tested locally; full `cargo test --workspace` stays
  a CI/builder gate.

## 7. REFACTOR notes

1. **Removed an unnecessary allocation.** `fetch` first did
   `self.active_view.as_str().to_string()`; `as_str()` returns `&'static str`, so
   the owned copy was pointless. It now borrows directly and passes `&str` to the
   bridge.
2. **Extracted `tests/common/mod.rs`.** The in-process mock daemon (3 service
   traits, TCP and UDS spawners) was inline in `test_grpc_bridge.rs`; TASK-03
   needs it too. It now lives in one shared test module used by both test binaries
   (with an explicit `#![allow(dead_code)]` because each test binary compiles its
   own copy).
3. **Atomic state swap.** `refresh` delegates to `fetch`, which awaits both RPCs
   before touching any field, so a mid-refresh failure cannot leave the UI with
   half-updated data; the status transition happens in one place only.
4. **`clamp_focus` runs after every refresh**, so a shrunken list cannot leave the
   focus pointing past the end.
5. Rules review: no `unwrap()`/`expect()` in `state.rs`, no `.clone()` beyond the
   three owned `String` fields the view model must hold, formatting helpers are
   pure functions, every `match` is exhaustive.

## 8. Design decisions worth reviewing

- **Units are binary (1024-based) with one decimal**, matching the plan's
  `"   0.0 KB"` and `"1.4"` expectations; the unit ladder is KB → TB.
- **`FocusIndex` is a type alias for `usize`**, so the plan's
  `assert_eq!(state.focus_index, 1)` contract holds while the type name documents
  intent.
- **`refresh` returns `Result<(), BridgeError>`** in addition to setting
  `Disconnected`, so TASK-04's loop can react (log, back off) without re-inspecting
  the state.
- **`selected()` returns `Option<&TorrentViewModel>`** rather than indexing, which
  removes a panic path before the widgets exist.

## 9. Changed files

| Action | Path | Notes |
|--------|------|-------|
| CREATE | `crates/rt-tui-client/src/state.rs` | `AppState`, `TorrentViewModel`, `SessionStats`, `ActiveView`, `ConnectionStatus` (296 lines) |
| CREATE | `crates/rt-tui-client/tests/test_app_state.rs` | 19 tests (292 lines) |
| CREATE | `crates/rt-tui-client/tests/common/mod.rs` | shared mock daemon (232 lines) |
| MODIFY | `crates/rt-tui-client/tests/test_grpc_bridge.rs` | now uses `common` instead of an inline mock |
| MODIFY | `crates/rt-tui-client/src/bridge.rs` | `session_stats()` added |
| MODIFY | `crates/rt-tui-client/src/lib.rs` | `pub mod state` + re-exports |

Commits:

| Commit | Stage |
|--------|-------|
| `3d94e0e8` | `test:` RED checkpoint (contract + shared mock extraction) |
| `7aaaf3bd` | `feat:` GREEN + refactor |

## 10. Not in scope (next plan task)

`TASK-04 — Event Loop & Terminal Backend` wires `AppState::refresh` into a
ratatui/crossterm loop with a bounded tick interval and replaces the one-shot
`Endpoint::probe()` in `run()`.
