# TDD Evidence — TASK-11 — Input Prompt & Command Execution

> **Source plan:** [`docs/tui-implementation.plan.md`](../tui-implementation.plan.md) — section *TASK-11 — Input Prompt & Command Execution*
> **Skill:** `skills/tdd-workflow` (RED → GREEN → REFACTOR)
> **Rules applied:** `rules/rust/coding-style.md`, `rules/rust/patterns.md`
> **Depends on:** TASK-10 (`7449f9aa`)
> **Date:** 2026-10-29
> **Note:** implemented together with TASK-12 and TASK-13; contracts written
> first, then each test file run and fixed in turn.

## 1. User journey (from plan)

> As a user, I press `^X` and a command prompt appears at the bottom. I type
> `d.priority.set=3`, press Enter, and the command executes via gRPC. I press
> Backspace to load a torrent by path.

| # | Acceptance criterion | Status |
|---|----------------------|--------|
| 1 | `InputWidget` renders a single-line input at the bottom | PASS |
| 2 | Prompt modes: `Command` (^X), `LoadStart` (Backspace), `LoadNormal`, `Filter` (/) | PASS (modes exist; see §7) |
| 3 | Cursor movement, delete, backspace, insert | PASS |
| 4 | `Enter` submits; `Escape`/`^G` cancels | PASS |
| 5 | Command submission goes through `CommandService` | PASS |
| 6 | Error display in the status bar on failure | PASS |
| 7 | Input history (Up/Down), bounded | PASS |

## 2. Validation commands (actually run)

| Gate | Command | Result |
|------|---------|--------|
| RED | `command cargo test -p rt-tui-client --test test_input_widget` | FAIL (expected) |
| GREEN | same command | PASS — 21 tests |
| Regression | all 15 test files, one at a time | PASS — 202 tests |
| Lint / format / check | `clippy --workspace --all-targets -D warnings`, `fmt --check`, `check --workspace --all-targets` | PASS |
| Coverage | `llvm-cov -p rt-tui-client` | 88.09 % lines (input.rs 81.88 %) |
| Manual E2E | pty 95×20: `^X` + `d.priority.set=3` + Enter | PASS (see §5) |

## 3. RED evidence

```text
error[E0432]: unresolved import `rt_tui_client::widgets::input`
error[E0432]: unresolved import `rt_tui_client::widgets::InputWidget`
error[E0308]: TuiAction::Execute.command is &'static str, prompt text is not static
```

Committed as RED checkpoint `edc7f093`.

## 4. GREEN evidence

```text
Running tests/test_input_widget.rs
test result: ok. 21 passed; 0 failed; 0 ignored
```

Failures fixed on the way to green:

1. `ctrl_x_opens_the_command_prompt` returned a throttle action instead of an
   `OpenPrompt`. Two causes: prompt keys were only reachable when a torrent was
   focused, and the throttle lookup ignored `CONTROL`. Both fixed
   (`key_handler::global_action`, modifier guard).
2. `command_history_survives_reopening_the_prompt` failed because history lived
   inside the `InputState`, which is dropped when the prompt closes. History now
   lives on `App` and seeds each new prompt.
3. The TASK-10 test that asserted `take_prompt_request()` was retargeted at
   `App::prompt` once prompts became real (the queue API was removed).

Committed as `08dabb20`.

## 5. Manual end-to-end run

pty 95×20, mock daemon, `^X`, typing `d.priority.set=3`, Enter:

```text
prompt line : d.priority.set=3▌
daemon log  : EXECUTE d.priority.set args=[ObjectValue { kind: Some(IntValue(3)) }] target=
```

## 6. Test specification

| # | What is guaranteed | Test | Result |
|---|--------------------|------|--------|
| 1 | Typed characters accumulate | `input_accepts_and_returns_text` | PASS |
| 2 | Home/End move the cursor | `input_cursor_movement` | PASS |
| 3 | Escape cancels | `input_escape_clears_and_returns_none` | PASS |
| 4 | Enter submits the text | `input_enter_returns_submitted_text` | PASS |
| 5 | Up walks history backwards | `input_history_navigates` | PASS |
| 6 | Backspace/Delete edit at the cursor | `backspace_and_delete_edit_at_the_cursor` | PASS |
| 7 | Insert happens at the cursor | `insert_happens_at_the_cursor` | PASS |
| 8 | `^G` cancels like Escape | `ctrl_g_cancels_like_escape` | PASS |
| 9 | Down restores the typed draft | `history_down_restores_the_draft` | PASS |
| 10 | History is bounded to 100 | `history_is_bounded` | PASS |
| 11 | Control keys do not insert text | `typing_ignores_control_keys` | PASS |
| 12 | The widget renders text and label | `widget_renders_prompt_label_and_text` | PASS |
| 13 | Each mode has its own label | `widget_uses_a_distinct_label_per_mode` | PASS |
| 14 | `^X` opens the command prompt | `ctrl_x_opens_the_command_prompt` | PASS |
| 15 | Escape closes the prompt | `escape_closes_the_prompt` | PASS |
| 16 | Enter parses `command=args` into an action | `typing_and_submitting_returns_a_command_action` | PASS |
| 17 | Bad arguments are rejected and reported | `invalid_command_text_does_not_submit_and_reports_it` | PASS |
| 18 | Load prompt yields a `Load` action | `load_prompt_produces_a_load_action` | PASS |
| 19 | Commands reach CommandService | `submitted_commands_reach_the_daemon` | PASS |
| 20 | History survives reopening | `command_history_survives_reopening_the_prompt` | PASS |
| 21 | Notices render in the status bar | `status_bar_shows_a_notice` | PASS |

## 7. Coverage and known gaps

```text
widgets/input.rs   259   50   80.69%    160   29   81.88%
TOTAL             3198  448   85.99%   2167  258   88.09%
```

Known gaps:

- **Only integer arguments are supported** (`d.priority.set=3`). Commands that
  need strings go through `TuiAction::Load` instead; a general string-argument
  path would need `TuiAction::Execute.args` to become a union of ints and text.
- **`LoadNormal` is reachable through the API but not bound to a key** — the plan
  only binds Backspace (LoadStart). `load.normal` is wired end to end.
- **`Filter` mode stores `AppState.filter` but the list does not apply it yet**;
  filtering the download list is TASK-16 scope.
- **The prompt is single-line with a per-character cursor**; no word-wise
  movement or clipboard, which the plan does not ask for.
- **Unused `PromptMode` label duplication** (`load:` for both load modes) is
  deliberate: the user only needs to know it is a load prompt.
- `input.rs` coverage is 81.9 % — the cursor-at-end rendering branch and some
  history edge cases are not asserted.

## 8. REFACTOR notes

1. **Prompt history hoisted to `App`** because state inside `InputState` dies with
   the prompt; `open_prompt` replays the stored lines.
2. **`handle_prompt_key` borrows the prompt only to read the result**, then drops
   the borrow before mutating `self.prompt` — avoids a double-mutable-borrow.
3. **`global_action` split out of `torrent_action`** so `^X`/Backspace work on an
   empty list (they are client-level).
4. **`TuiAction::Execute.command` became `String`** (prompt input is dynamic);
   TASK-10 tests moved from literal patterns to guards.
5. Rules review: no `unwrap()`/`expect()` in `src/`, cursor arithmetic goes
   through `byte_index` (no byte/char slicing panics), history is bounded.

## 9. Design decisions worth reviewing

- **`command=1,2` parsing** matches rTorrent's CLI syntax and keeps the action
  type simple; invalid tokens produce a visible notice instead of a silent drop.
- **Notices are transient state, not a log**: the next successful action clears
  them.
- **The prompt occupies its own row** above the status bar, so the list does not
  jump when it opens/closes.

## 10. Changed files

| Action | Path | Notes |
|--------|------|-------|
| CREATE | `crates/rt-tui-client/src/widgets/input.rs` | 297 lines |
| CREATE | `crates/rt-tui-client/tests/test_input_widget.rs` | 21 tests (350 lines) |
| MODIFY | `crates/rt-tui-client/src/app.rs` | prompt routing, parsing, history, `set_prompt_text` |
| MODIFY | `crates/rt-tui-client/src/key_handler.rs` | `global_action` |
| MODIFY | `crates/rt-tui-client/src/state.rs` | `notice`, `filter` |
| MODIFY | `crates/rt-tui-client/src/widgets/status_bar.rs` | notice line |
| MODIFY | `crates/rt-tui-client/src/actions.rs`, `bridge.rs`, `lib.rs` | `Execute.command: String`, `Load`, `execute_strings` |

Commits: RED `edc7f093`, GREEN `08dabb20`.

## 11. Not in scope

Applying `AppState.filter` to the download list (TASK-16) and general
string-argument commands.
