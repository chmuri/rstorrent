# TDD Evidence — TASK-12 — Context Menus

> **Source plan:** [`docs/tui-implementation.plan.md`](../tui-implementation.plan.md) — section *TASK-12 — Context Menus*
> **Skill:** `skills/tdd-workflow` (RED → GREEN → REFACTOR)
> **Rules applied:** `rules/rust/coding-style.md`, `rules/rust/patterns.md`
> **Depends on:** TASK-10 (`7449f9aa`), TASK-11 (`08dabb20`)
> **Date:** 2026-10-29
> **Note:** implemented together with TASK-11 and TASK-13; contracts written first,
> then each test file run and fixed in turn.

## 1. User journey (from plan)

> As a user, I press `m` on a torrent and a context menu appears with: Start,
> Stop, Close, Check Hash, Set Priority, etc. I navigate with Up/Down and press
> Enter to execute.

| # | Acceptance criterion | Status |
|---|----------------------|--------|
| 1 | `MenuWidget` renders as a popup overlay | PASS (centred, cleared, bordered) |
| 2 | Menu entries: label, action, enabled flag | PASS |
| 3 | Navigation: Up/Down, Enter executes, Escape closes | PASS |
| 4 | Disabled entries visible but not selectable | PASS (dimmed, skipped) |
| 5 | Action execution through the same `TuiAction` path | PASS |
| 6 | Menu adapts to available screen height | PASS (windows around the selection) |

## 2. Validation commands (actually run)

| Gate | Command | Result |
|------|---------|--------|
| RED | `command cargo test -p rt-tui-client --test test_menu_widget` | FAIL (expected) |
| GREEN | same command | PASS — 12 tests |
| Regression | all 15 test files, one at a time | PASS — 202 tests |
| Lint / format / check | `clippy --workspace --all-targets -D warnings`, `fmt --check`, `check --workspace --all-targets` | PASS |
| Coverage | `llvm-cov -p rt-tui-client` | 88.09 % lines (menu.rs 97.73 %) |
| Manual E2E | pty 95×20: `m` → down → Enter | PASS (see §5) |

## 3. RED evidence

```text
error[E0432]: unresolved imports `rt_tui_client::state::MenuEntry`,
              `rt_tui_client::state::MenuResult`, `rt_tui_client::state::MenuState`
error[E0432]: unresolved import `rt_tui_client::widgets::MenuWidget`
```

Committed as RED checkpoint `edc7f093`.

## 4. GREEN evidence

```text
Running tests/test_menu_widget.rs
test result: ok. 12 passed; 0 failed; 0 ignored
```

Committed as `08dabb20`.

## 5. Manual end-to-end run

pty 95×20, mock daemon, `m` then Down then Enter:

```text
┌ menu ────────────┐
│ Start            │   (disabled: torrent is running → dimmed)
│ Stop             │   (focused, reversed)
│ Close            │
│ Check Hash       │
│ Priority Low     │
│ Priority Normal  │
│ Priority High    │
│ Remove           │
└──────────────────┘

daemon log: CONTROL hash=abc123def456 action=5   (Close, after Down)
```

## 6. Test specification

| # | What is guaranteed | Test | Result |
|---|--------------------|------|--------|
| 1 | Entries render inside the popup | `menu_renders_entries` | PASS |
| 2 | `activate()` on a disabled entry yields nothing | `menu_skips_disabled_on_enter` | PASS |
| 3 | Escape closes the menu | `menu_escape_closes` | PASS |
| 4 | Down/Up skip disabled entries | `navigation_skips_disabled_entries` | PASS |
| 5 | Enter on an enabled entry returns its action | `enter_activates_the_selected_entry` | PASS |
| 6 | The focused row is reversed | `selected_row_is_highlighted` | PASS |
| 7 | Tall menus window around the selection | `widget_fits_tall_lists_into_the_screen` | PASS |
| 8 | `m` opens a menu for the focused torrent | `m_opens_a_menu_for_the_focused_torrent` | PASS |
| 9 | `m` without a torrent does nothing | `menu_is_not_opened_without_a_torrent` | PASS |
| 10 | Enter returns the action and closes the menu | `menu_enter_returns_the_entry_action_and_closes` | PASS |
| 11 | Escape closes without an action | `menu_escape_closes_without_an_action` | PASS |
| 12 | Entries reflect the torrent state (Start/Stop enablement) | `menu_entries_reflect_the_torrent_state` | PASS |

## 7. Coverage and known gaps

```text
widgets/menu.rs    88    1   98.86%      44    1   97.73%
TOTAL            3198  448   85.99%    2167  258   88.09%
```

Known gaps:

- **The menu is not scrollable with PgUp/PgDn** — Up/Down wrap, and the widget
  windows around the cursor, which is enough for eight entries.
- **"Remove + Data" is not offered** — `RemoveTorrent` always keeps data
  (`delete_data: false`); a destructive variant needs a confirmation flow.
- **The menu does not close on `q`** (only Escape/Enter); `^Q` still quits the
  whole client because that check runs first.
- **Priority entries are disabled when already active**, so the menu shows
  current state rather than hiding it.

## 8. REFACTOR notes

1. **`MenuState` lives in `state.rs`** as the plan says, keeping the widget purely
   presentational; `handle_key` returns `MenuResult` so the App decides.
2. **`handle_menu_key` copies the result out of the borrow** before mutating
   `self.state.menu`, avoiding a double mutable borrow.
3. **`menu_entries(torrent)` is a free function** so the entry set can be
   asserted without opening a menu.
4. **Reused `Clear` + `viewport`** from ratatui/download_list instead of writing
   custom overlay or scrolling code.

## 9. Design decisions worth reviewing

- **Disabled entries stay visible and dimmed** — the user sees what is possible,
  not a menu that changes shape.
- **Selection wraps** and always lands on an enabled entry, so Enter is never a
  dead key.
- **The popup is centred and sized from the longest label**, clamped to the body
  area so it never overlaps the title/status bars.

## 10. Changed files

| Action | Path | Notes |
|--------|------|-------|
| CREATE | `crates/rt-tui-client/src/widgets/menu.rs` | 76 lines |
| CREATE | `crates/rt-tui-client/tests/test_menu_widget.rs` | 12 tests (239 lines) |
| MODIFY | `crates/rt-tui-client/src/state.rs` | `MenuEntry`, `MenuState`, `MenuResult`, `AppState.menu` |
| MODIFY | `crates/rt-tui-client/src/app.rs` | `m` binding, menu routing, `menu_entries`, overlay rendering |
| MODIFY | `crates/rt-tui-client/src/actions.rs` | `TuiAction::Noop` for inert entries |

Commits: RED `edc7f093`, GREEN `08dabb20`.

## 11. Not in scope

Destructive remove-with-data and per-file context menus.
