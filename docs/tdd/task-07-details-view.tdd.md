# TDD Evidence — TASK-07 — Torrent Details View

> **Source plan:** [`docs/tui-implementation.plan.md`](../tui-implementation.plan.md) — section *TASK-07 — Torrent Details View*
> **Skill:** `skills/tdd-workflow` (RED → GREEN → REFACTOR)
> **Rules applied:** `rules/rust/coding-style.md`, `rules/rust/patterns.md`
> **Depends on:** TASK-02 (`de890203`), TASK-05 (`a63156e3`)
> **Date:** 2026-10-29

## 1. User journey (from plan)

> As a user, I press Right (or `l` in vi mode) on a torrent to enter details. I
> see tabs: Info / Files / Peers / Trackers / Chunks. I navigate tabs with
> Left/Right or 1-5. I press Left (or `h`) to return to the list.

| # | Acceptance criterion | Status |
|---|----------------------|--------|
| 1 | `DetailsWidget` fetches `TorrentDetails` via the bridge on activation | PASS (`App::open_details`, driven by the run loop) |
| 2 | Tab bar `[1:Info] [2:Files] [3:Peers] [4:Trackers] [5:Chunks]` | PASS |
| 3 | Info tab: hash, name, size, piece length/count, directory, message, private, connection_seed, throttle | PASS |
| 4 | Navigation: Left/`h`/`^B` returns to the download list | PASS (Left returns from the first tab — see §9) |
| 5 | Tab switching: number keys 1–5, Left/Right arrows | PASS |

## 2. Validation commands (actually run)

Remote builder still unreachable → local escape hatch (`command cargo ...`).

| Gate | Command | Result |
|------|---------|--------|
| RED | `command cargo test -p rt-tui-client --test test_details_widget` | FAIL (expected) |
| GREEN | `command cargo test -p rt-tui-client` | PASS — 114 tests (18 details) |
| Lint | `command cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| Format | `command cargo fmt --check` | PASS |
| Compile gate | `command cargo check --workspace --all-targets` | PASS |
| Coverage | `command cargo llvm-cov -p rt-tui-client --summary-only --ignore-filename-regex 'main\.rs'` | 89.45 % lines |
| Manual E2E | pty 90×18: `l` → details, `3` → Peers, `h` → list, `^Q` | PASS (see §5) |

## 3. RED evidence

```text
error[E0432]: unresolved imports `rt_tui_client::state::DetailTab`,
              `rt_tui_client::state::DetailsState`
  --> crates/rt-tui-client/tests/test_details_widget.rs:15:38

error[E0432]: unresolved import `rt_tui_client::widgets::DetailsWidget`
  --> crates/rt-tui-client/tests/test_details_widget.rs:16:20

error[E0609]: no field `details` on type `AppState`
error[E0599]: no method named `take_details_request` found for struct `App`
```

Committed as RED checkpoint `843b5817`.

## 4. GREEN evidence

```text
Running tests/test_app_state.rs            test result: ok. 21 passed
Running tests/test_bars.rs                 test result: ok. 12 passed
Running tests/test_cli.rs                  test result: ok. 5 passed
Running tests/test_details_widget.rs       test result: ok. 18 passed
Running tests/test_download_list_widget.rs test result: ok. 12 passed
Running tests/test_endpoint.rs             test result: ok. 13 passed
Running tests/test_event_loop.rs           test result: ok. 15 passed
Running tests/test_grpc_bridge.rs          test result: ok. 15 passed
Running tests/test_run.rs                  test result: ok. 3 passed
```

Committed as `a12b602b`.

## 5. Manual end-to-end run

Mock daemon serving one torrent plus real `GetTorrent` details, pty at 90×18:

```text
list    rstorrent v0.16.1 — [main] 1 torrent
        Name              Size    Done Progress  ↓ Rate  ↑ Rate  State        Ratio
        ubuntu-26.04.iso  2.0 GB   50% ████░░░░  1.4 MB   0.0 KB downloading  0.000
        ↑ 146.5 KB ↓ 1.4 MB  thr ↑ ∞ ↓ ∞  :6881  ● connected

press l [1:Info] [2:Files] [3:Peers] [4:Trackers] [5:Chunks]
        Hash          abc123def456
        Name          ubuntu-26.04.iso
        State         downloading
        Size          2.0 GB
        Piece length  256.0 KB
        Pieces        8192
        Directory     /downloads/linux
        Private       yes
        Seed          seed-1
        Throttle      main
        Message       Tracker: working

press 3 [1:Info] [2:Files] [3:Peers] [4:Trackers] [5:Chunks]   (Peers inverted)
        Peers: 0 entries

press h back to the list; ^Q exited 0
```

## 6. Test specification

| # | What is guaranteed | Test | Type | Result |
|---|--------------------|------|------|--------|
| 1 | Info tab renders hash and name | `test_details_widget.rs:details_info_tab_shows_hash_and_name` | unit | PASS |
| 2 | `next_tab()` walks Info → Files → Peers | `tab_navigation_cycles_correctly` | unit | PASS |
| 3 | The tab bar shows all five numbered labels | `tab_bar_shows_all_five_labels` | unit | PASS |
| 4 | The active tab is inverted and inactive ones are not | `active_tab_is_highlighted` | unit | PASS |
| 5 | `previous_tab()` wraps Info → Chunks | `previous_tab_wraps_to_chunks` | unit | PASS |
| 6 | Info shows piece length/count, directory, privacy, seed, throttle, size | `info_tab_shows_pieces_directory_privacy_and_throttle` | unit | PASS |
| 7 | Files/Peers tabs render their entry counts | `non_info_tabs_report_entry_counts` | unit | PASS |
| 8 | Trackers/Chunks tabs render their counts | `trackers_and_chunks_tabs_render_their_counts` | unit | PASS |
| 9 | Empty daemon strings fall back to `—` | `missing_summary_fields_fall_back_to_dashes` | unit | PASS |
| 10 | `l`/Right request details for the focused torrent, once | `l_and_right_request_details_for_the_selected_torrent` | unit (App) | PASS |
| 11 | `l` on an empty list requests nothing | `l_on_an_empty_list_requests_nothing` | unit (App) | PASS |
| 12 | `h` returns to the list | `h_returns_to_the_download_list` | unit (App) | PASS |
| 13 | `^B` returns to the list | `ctrl_b_returns_to_the_download_list` | unit (App) | PASS |
| 14 | `1`–`5` select a tab | `number_keys_select_a_tab` | unit (App) | PASS |
| 15 | Left steps back a tab, and leaves from Info | `left_moves_back_a_tab_and_leaves_from_info` | unit (App) | PASS |
| 16 | `open_details` populates the pane from a live daemon | `open_details_loads_the_daemon_details` | integration (in-process daemon) | PASS |
| 17 | A headless app cannot open details | `headless_app_cannot_open_details` | unit (App) | PASS |
| 18 | `App::draw` renders the details pane when open | `draw_renders_the_details_view_when_open` | unit (`TestBackend`) | PASS |

## 7. Coverage and known gaps

```text
Filename                    Regions  Missed  Cover   Lines  Missed  Cover
state.rs                        310       8  97.42%     223       5  97.76%
widgets/details.rs              128       1  99.22%      67       0 100.00%
TOTAL                          1423     167  88.26%     957     101  89.45%
```

Known gaps:

- **"message" is daemon-provided** (`TorrentSummary.message`), not a separate
  proto field; when the daemon sends nothing the row shows `—`. Verified against
  the real proto rather than inventing a field.
- **Files/Peers/Trackers/Chunks tabs only show entry counts.** Real lists are
  TASK-08 (files) and TASK-09 (peers/trackers). No piece map exists in the proto,
  so Chunks reports the piece count until a bitmap RPC exists.
- **Tabs are not scrollable yet** — with more entries than rows, TASK-08/09 add
  their own viewports.
- **`open_details` failure only logs** (`tracing::warn!`) and leaves the list
  visible; surfacing it in the status bar belongs to TASK-15.
- Only the affected crate is tested locally; full `cargo test --workspace` stays
  a CI/builder gate.

## 8. REFACTOR notes

1. **Details view model lives in `state.rs`, not in the widget file.** The plan
   lists `DetailsState` under `widgets/details.rs` but also says to add it to
   `AppState`; keeping it beside `TorrentViewModel`/`SessionStats` preserves the
   crate's "widgets never format or compute, they only draw" rule.
2. **`handle_key` split into list/details handlers** after the `^Q` check, so the
   two key maps cannot interfere and neither needs a mode parameter.
3. **The details fetch is a request/consume pair** (`details_request` +
   `take_details_request()`): `handle_key` stays synchronous, the run loop awaits
   `open_details`, and the test can observe the request without a daemon.
4. **`info_line` borrows values** (`Line<'a>` from `&str`) so the Info body does
   not clone the view model every frame; only the bold labels allocate.
5. Rules review: no `unwrap()`/`expect()` in `src/`, `unwrap_or_default`/`or_dash`
   for absent proto data, exhaustive `match` on `DetailTab` in both the widget and
   the tab bar.

## 9. Design decisions worth reviewing

- **Left is dual-purpose by design.** The plan says both "navigate tabs with
  Left/Right" and "press Left to return"; the implementation resolves it as
  "Left steps back one tab, and from the first tab it leaves the pane", so both
  statements hold and `h`/`^B` remain unambiguous exits.
- **`l` opens details from the list and advances tabs inside details** (vi
  symmetry with `h`).
- **Details replace the list body**, keeping the title and status bars mounted —
  no second full-screen mode to keep in sync.
- **Counts-only placeholder tabs** instead of "TODO" strings, so the UI is always
  meaningful while TASK-08/09 land.

## 10. Changed files

| Action | Path | Notes |
|--------|------|-------|
| CREATE | `crates/rt-tui-client/src/widgets/details.rs` | `DetailsWidget` (112 lines) |
| CREATE | `crates/rt-tui-client/tests/test_details_widget.rs` | 18 tests (349 lines) |
| MODIFY | `crates/rt-tui-client/src/state.rs` | `DetailTab`, `DetailsInfo`, `DetailsState`, `AppState::details` |
| MODIFY | `crates/rt-tui-client/src/app.rs` | mode-aware keys, `take_details_request`, `open_details`, details rendering |
| MODIFY | `crates/rt-tui-client/src/widgets/mod.rs`, `src/lib.rs` | module wiring + re-exports |

Commits:

| Commit | Stage |
|--------|-------|
| `843b5817` | `test:` RED checkpoint |
| `a12b602b` | `feat:` GREEN + refactor |

## 11. Not in scope (next plan task)

`TASK-08 — File List View` fills the Files tab with per-file rows, priorities and
a scrolling viewport.
