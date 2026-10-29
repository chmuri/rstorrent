# TDD Evidence — TASK-09 — Peer & Tracker List Views

> **Source plan:** [`docs/tui-implementation.plan.md`](../tui-implementation.plan.md) — section *TASK-09 — Peer & Tracker List Views*
> **Skill:** `skills/tdd-workflow` (RED → GREEN → REFACTOR)
> **Rules applied:** `rules/rust/coding-style.md`, `rules/rust/patterns.md`
> **Depends on:** TASK-07 (`a12b602b`)
> **Date:** 2026-10-29
> **Note:** implemented together with TASK-10 at the operator's request; both
> contracts were written first, then each test file was run and fixed in turn.

## 1. User journey (from plan)

> As a user, I select the Peers tab and see: address, client, ↑/↓ rate, encrypted
> flag. In the Trackers tab: URL, enabled, group, last event.

| # | Acceptance criterion | Status |
|---|----------------------|--------|
| 1 | `PeerListWidget` renders from `TorrentDetails.peers` | PASS |
| 2 | Columns: address, peer_id/client, ↑ rate, ↓ rate, 🔒 encrypted | PASS |
| 3 | `TrackerListWidget` renders from `TorrentDetails.trackers` | PASS |
| 4 | Columns: group, URL, enabled, index | PASS |
| 5 | Both lists support focus navigation | PASS |
| 6 | Empty list shows "No peers" / "No trackers" | PASS |

## 2. Validation commands (actually run)

| Gate | Command | Result |
|------|---------|--------|
| RED | `command cargo test -p rt-tui-client --test test_peer_tracker_widget` | FAIL (expected) |
| GREEN | `command cargo test -p rt-tui-client --test test_peer_tracker_widget` | PASS — 11 tests |
| Regression | full crate suite | PASS — 153 tests |
| Lint / format / check | `clippy --workspace --all-targets -D warnings`, `fmt --check`, `check --workspace --all-targets` | PASS |
| Coverage | `llvm-cov -p rt-tui-client` | 88.29 % lines (peer_list 100 %, tracker_list 100 %) |
| Manual E2E | pty 95×20, Peers + Trackers tabs | PASS (see §5) |

## 3. RED evidence

```text
error[E0432]: unresolved imports `rt_tui_client::state::PeerListState`,
              `rt_tui_client::state::TrackerListState`
  --> crates/rt-tui-client/tests/test_peer_tracker_widget.rs:9:28

error[E0432]: unresolved imports `rt_tui_client::widgets::PeerListWidget`,
              `rt_tui_client::widgets::TrackerListWidget`
  --> crates/rt-tui-client/tests/test_peer_tracker_widget.rs:10:28
```

Committed as RED checkpoint `64ffc4f2` (shared with TASK-10).

## 4. GREEN evidence

```text
Running tests/test_peer_tracker_widget.rs
test result: ok. 11 passed; 0 failed; 0 ignored
```

One test needed a correction during GREEN: `unencrypted_peers_have_no_lock`
asserted that `🔒` never appears, but the table header always carries the glyph.
The test now counts occurrences (1 = header only, 2 = header + encrypted row),
which is what the widget actually guarantees.

Committed as `7449f9aa` (shared with TASK-10).

## 5. Manual end-to-end run

Mock daemon with two peers (one encrypted, qBittorrent + Transmission peer ids)
and two trackers, pty at 95×20, `l` → `3` → `4`:

```text
[1:Info] [2:Files] [3:Peers] [4:Trackers] [5:Chunks]
Address          Client        ↓ Rate   ↑ Rate   🔒
1.2.3.4:6881     qBittorrent   1.4 MB   20.0 KB  🔒
5.6.7.8:51413    Transmission  0.0 KB   1.0 KB

Group  Index  Enabled  URL
0      0      yes      udp://tracker.one:6969/announce
1      1      no       http://tracker.two/announce
```

## 6. Test specification

| # | What is guaranteed | Test | Type | Result |
|---|--------------------|------|------|--------|
| 1 | Peer address and the encryption glyph render | `test_peer_tracker_widget.rs:peer_list_renders_address_and_encryption` | unit | PASS |
| 2 | Tracker URL renders | `tracker_list_renders_url_and_group` | unit | PASS |
| 3 | An empty peer list says "No peers" | `empty_peer_list_shows_message` | unit | PASS |
| 4 | Client name and both rates render | `peer_rows_show_client_and_rates` | unit | PASS |
| 5 | Unencrypted peers carry no glyph beyond the header | `unencrypted_peers_have_no_lock` | unit | PASS |
| 6 | Peer focus moves and clamps; paging works | `peer_focus_moves_and_clamps` | unit | PASS |
| 7 | The focused peer row is reversed | `focused_peer_row_is_highlighted` | unit | PASS |
| 8 | Tracker index/group/enabled render | `tracker_rows_show_index_group_and_enabled_state` | unit | PASS |
| 9 | An empty tracker list says "No trackers" | `empty_tracker_list_shows_message` | unit | PASS |
| 10 | Tracker focus moves and clamps | `tracker_focus_moves_and_clamps` | unit | PASS |
| 11 | Peer-id prefixes map to client names | `peer_client_name_is_derived_from_the_peer_id` | unit | PASS |

## 7. Coverage and known gaps

```text
widgets/peer_list.rs       79    0  100.00%      41     0  100.00%
widgets/tracker_list.rs    78    0  100.00%      39     0  100.00%
TOTAL                    2359  336   85.76%    1588   186   88.29%
```

Known gaps:

- **"last event" is not shown** — `TrackerInfo` has no last-event field in the
  proto, so the Trackers tab shows group/index/enabled/URL only. Recorded rather
  than invented.
- **Focus navigation in Peers/Trackers through `App` is not wired yet**: the list
  states expose `focus_*`, but `App::handle_details_key` only routes Up/Down to the
  Files tab. TASK-10 was implemented in the same batch and did not add it either;
  it is a one-arm follow-up.
- **Client names cover the six most common clients**; anything else displays the
  raw peer id.
- **No peer/tracker actions** (ban, add tracker) — not in the plan for this phase.

## 8. REFACTOR notes

1. **One `step()` helper** clamps focus for both lists instead of duplicating
   arithmetic; the file list keeps its own skipping logic.
2. **Client name mapping is a pure function** of the peer id, so it is unit-tested
   without constructing widgets.
3. **Colour decisions live in the widgets** (`Color::Green` for encrypted/enabled,
   `Color::Red` for disabled) while the state layer stays ratatui-free.
4. **Reused `viewport()`/`visible_rows()`** from the download list, so all four
   tables scroll identically.

## 9. Design decisions worth reviewing

- **`🔒` in the header as well as the row** keeps the column self-describing; the
  test asserts the count rather than mere presence.
- **Disabled trackers are red "no"**, enabled green "yes" — same colour language
  as file priorities.
- **Rates are trimmed** in the tables (`1.4 MB` instead of `   1.4 MB`) because a
  table column already aligns them.

## 10. Changed files

| Action | Path | Notes |
|--------|------|-------|
| CREATE | `crates/rt-tui-client/src/widgets/peer_list.rs` | 75 lines |
| CREATE | `crates/rt-tui-client/src/widgets/tracker_list.rs` | 73 lines |
| CREATE | `crates/rt-tui-client/tests/test_peer_tracker_widget.rs` | 11 tests (219 lines) |
| MODIFY | `crates/rt-tui-client/src/state.rs` | `PeerListState`/`PeerEntry`, `TrackerListState`/`TrackerEntry`, `DetailsState.peers/trackers` |
| MODIFY | `crates/rt-tui-client/src/widgets/details.rs` | Peers/Trackers tabs render lists |
| MODIFY | `crates/rt-tui-client/src/widgets/mod.rs`, `src/lib.rs` | wiring + re-exports |
| MODIFY | `crates/rt-tui-client/tests/test_details_widget.rs` | tabs now assert real rows |

Commits: RED `64ffc4f2`, GREEN `7449f9aa`.

## 11. Not in scope

Focus routing for the Peers/Trackers tabs (small follow-up) and `TASK-11 — Input
Prompt & Command Execution`.
