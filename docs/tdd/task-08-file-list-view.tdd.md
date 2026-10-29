# TDD Evidence — TASK-08 — File List View

> **Source plan:** [`docs/tui-implementation.plan.md`](../tui-implementation.plan.md) — section *TASK-08 — File List View*
> **Skill:** `skills/tdd-workflow` (RED → GREEN → REFACTOR)
> **Rules applied:** `rules/rust/coding-style.md`, `rules/rust/patterns.md`
> **Depends on:** TASK-07 (`a12b602b`)
> **Date:** 2026-10-29

## 1. User journey (from plan)

> As a user, I select the Files tab in details and see per-file: path, size,
> completed %, priority. I can change priority with Space/`*` and navigate with
> Up/Down.

| # | Acceptance criterion | Status |
|---|----------------------|--------|
| 1 | `FileListWidget` renders rows from `TorrentDetails.files` | PASS |
| 2 | Columns: index, path, size, completed, priority (Off/Normal/High) | PASS |
| 3 | Priority display with colour coding | PASS (red / green / yellow) |
| 4 | Focus/selection with Up/Down/PgUp/PgDn | PASS |
| 5 | Space/`*` cycles priority via `CommandService.Execute("f.priority.set", …)` | PASS |
| 6 | Directory rows for a shared prefix are synthetic and non-actionable | PASS |

## 2. Validation commands (actually run)

Remote builder still unreachable → local escape hatch (`command cargo ...`).

| Gate | Command | Result |
|------|---------|--------|
| RED | `command cargo test -p rt-tui-client --test test_file_list_widget` | FAIL (expected) |
| GREEN | `command cargo test -p rt-tui-client` | PASS — 127 tests (13 file-list) |
| Lint | `command cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| Format | `command cargo fmt --check` | PASS |
| Compile gate | `command cargo check --workspace --all-targets` | PASS |
| Coverage | `command cargo llvm-cov -p rt-tui-client --summary-only --ignore-filename-regex 'main\.rs'` | 89.67 % lines |
| Manual E2E | pty 90×16: Files tab, Space on the focused file | PASS (see §5) |

## 3. RED evidence

```text
error[E0432]: unresolved import `rt_tui_client::app::PriorityCommand`
error[E0432]: unresolved import `rt_tui_client::state::FileListState`
error[E0432]: unresolved import `rt_tui_client::widgets::file_list`
error[E0609]: no field `files` on type `&DetailsState`
```

Committed as RED checkpoint `b7a75d96`.

**Plan deviation (recorded):** the plan's snippet asserts both
`state.next_priority(0) == 2` ("normal → high") and
`state.next_priority_from(0) == 1` ("off → normal"). With 0 = off, 1 = normal,
2 = high those cannot both hold. The contract implements one coherent cycle —
`0 → 1 → 2 → 0` — and tests it; the contradictory line was not silently copied.

## 4. GREEN evidence

```text
Running tests/test_app_state.rs            test result: ok. 21 passed
Running tests/test_bars.rs                 test result: ok. 12 passed
Running tests/test_cli.rs                  test result: ok. 5 passed
Running tests/test_details_widget.rs       test result: ok. 18 passed
Running tests/test_download_list_widget.rs test result: ok. 12 passed
Running tests/test_endpoint.rs             test result: ok. 13 passed
Running tests/test_event_loop.rs           test result: ok. 15 passed
Running tests/test_file_list_widget.rs     test result: ok. 13 passed
Running tests/test_grpc_bridge.rs          test result: ok. 15 passed
Running tests/test_run.rs                  test result: ok. 3 passed
```

Committed as `5edac64c`.

Two failures were fixed before GREEN, both caused by the new f64 field:
`DetailsState` could no longer derive `Eq` (dropped), and the TASK-07 test that
asserted a "Files: N entries" placeholder was retargeted at the real file rows.

## 5. Manual end-to-end run

Mock daemon with three files under `data/`, pty at 90×16, `l` → `2` → Space:

```text
[1:Info] [2:Files] [3:Peers] [4:Trackers] [5:Chunks]
#   Path              Size      Done  Priority
    data/             1.0 GB
0   ubuntu-26.04.iso  1.0 GB    100%  High        <- after Space (was Normal)
1   README.txt        2.0 KB     50%  Normal
2   notes.md          4.0 KB      0%  Off
```

Daemon log for the same run:

```text
command: "f.priority.set" args=[IntValue(0), IntValue(2)] target="abc123def456"
```

So the keystroke reached `CommandService.Execute` with the file index, the new
priority and the torrent hash, as the acceptance criterion requires.

## 6. Test specification

| # | What is guaranteed | Test | Type | Result |
|---|--------------------|------|------|--------|
| 1 | Paths and priority labels render | `test_file_list_widget.rs:renders_file_path_and_priority` | unit | PASS |
| 2 | Priority cycles 0→1→2→0 | `priority_cycles_through_values` | unit | PASS |
| 3 | Labels are Off/Normal/High and colours red/green/yellow | `priority_labels_and_colours_differ` | unit | PASS |
| 4 | Size and completed % columns render | `rows_show_size_and_completed_percent` | unit | PASS |
| 5 | A shared prefix becomes one directory row and is stripped from paths | `shared_prefix_becomes_a_synthetic_directory_row` | unit | PASS |
| 6 | Files without a shared prefix get no directory row | `files_without_shared_prefix_have_no_directory_row` | unit | PASS |
| 7 | Focus starts on a file and skips the directory row | `focus_starts_on_the_first_file_and_skips_directories` | unit | PASS |
| 8 | Paging moves by a page and clamps at the last file | `paging_moves_focus_by_a_page_and_clamps` | unit | PASS |
| 9 | Space cycles the focused file and queues the command | `space_cycles_the_focused_files_priority_and_requests_a_command` | unit (App) | PASS |
| 10 | `*` also cycles priority | `star_also_cycles_priority` | unit (App) | PASS |
| 11 | Space on a directory row queues nothing | `space_on_a_directory_row_does_nothing` | unit (App) | PASS |
| 12 | Up/Down move the file focus inside the Files tab | `arrow_keys_move_the_file_focus_in_the_files_tab` | unit (App) | PASS |
| 13 | The priority command reaches CommandService | `priority_command_is_sent_through_the_bridge` | integration (in-process daemon) | PASS |
| 14 | The Files tab renders the list, not a placeholder | `test_details_widget.rs:files_tab_renders_the_file_list_and_peers_its_count` | unit | PASS |

## 7. Coverage and known gaps

```text
Filename                    Regions  Missed  Cover   Lines  Missed  Cover
state.rs                        486      13  97.33%     364       8  97.80%
widgets/file_list.rs            109       5  95.41%      57       2  96.49%
TOTAL                          1792     217  87.89%    1220     126  89.67%
```

Known gaps:

- **Priority 3+ (proto allows only 0/1/2)** is treated as `High` for display and
  cycles to `Off`; the daemon only sends 0–2 today.
- **Only the first shared directory level becomes a row** — deeper nesting stays
  flattened into the path. A real tree needs a prefix trie; the plan only asks
  for a synthetic row when files share a prefix.
- **Priority changes are optimistic**: the row updates before the daemon confirms.
  A rejected command is only logged (`tracing::warn!`); reverting on failure
  belongs with the error surfacing in TASK-15.
- **vi-style `j`/`k` are not bound** in the file list yet (TASK-10 owns the full
  key map); arrows and PgUp/PgDn work.
- Only the affected crate is tested locally; full `cargo test --workspace` stays
  a CI/builder gate.

## 8. REFACTOR notes

1. **Reused the viewport helper** from the download list instead of duplicating
   scroll arithmetic, so both lists behave identically.
2. **`priority_style` is a free function** returning a `Style`, which keeps colour
   decisions out of the state layer and makes them directly assertable.
3. **`FileEntry::priority_label` derives the label from the number** rather than
   storing it, so label and priority can never drift apart.
4. **`focus_down`/`focus_up` skip non-actionable rows by scanning** the entry
   vector, keeping `focus_index` a plain index into `entries` (no parallel list).
5. **Command plumbing mirrors the details request**: `handle_key` stays sync and
   queues a `PriorityCommand`, `run()` awaits `send_priority`, and tests observe
   the request without a daemon.
6. Rules review: no `unwrap()`/`expect()` in `src/`, `unwrap_or`/`unwrap_or_default`
   for absent data, exhaustive `match` on `DetailTab`, no per-frame clones of the
   file list (cells borrow `&str`; only index/percent/priority labels allocate).

## 9. Design decisions worth reviewing

- **Directory row only when ≥2 files share a prefix**, otherwise files render
  flat — a single-file torrent does not get a pointless tree.
- **Focus never lands on a directory row**, and Space there is a no-op: the row is
  presentational.
- **`f.priority.set` args are `(file index, priority)`** with the torrent hash as
  the command target, matching rTorrent's file-priority semantics.
- **Optimistic UI** for priority changes: the list is the user's feedback loop,
  and a round-trip per keystroke would feel laggy.
- **`DetailsState` lost `Eq`** because completion percentages are floats; `PartialEq`
  is what the tests need.

## 10. Changed files

| Action | Path | Notes |
|--------|------|-------|
| CREATE | `crates/rt-tui-client/src/widgets/file_list.rs` | `FileListWidget`, `priority_style` (96 lines) |
| CREATE | `crates/rt-tui-client/tests/test_file_list_widget.rs` | 13 tests (318 lines) |
| MODIFY | `crates/rt-tui-client/src/state.rs` | `FileEntry`, `FileListState`, `DetailsState.files` |
| MODIFY | `crates/rt-tui-client/src/app.rs` | `PriorityCommand`, file keys, `send_priority`, run-loop wiring |
| MODIFY | `crates/rt-tui-client/src/widgets/details.rs` | Files tab renders the list |
| MODIFY | `crates/rt-tui-client/src/widgets/mod.rs`, `src/lib.rs` | module wiring + re-exports |
| MODIFY | `crates/rt-tui-client/tests/test_details_widget.rs` | Files-tab assertion follows the real rows |

Commits:

| Commit | Stage |
|--------|-------|
| `b7a75d96` | `test:` RED checkpoint |
| `5edac64c` | `feat:` GREEN + refactor |

## 11. Not in scope (next plan task)

`TASK-09 — Peer & Tracker List Views` fills the remaining details tabs.
