# TDD Evidence — TASK-10 — Keyboard Shortcuts & Torrent Actions

> **Source plan:** [`docs/tui-implementation.plan.md`](../tui-implementation.plan.md) — section *TASK-10 — Keyboard Shortcuts & Torrent Actions*
> **Skill:** `skills/tdd-workflow` (RED → GREEN → REFACTOR)
> **Rules applied:** `rules/rust/coding-style.md`, `rules/rust/patterns.md`
> **Depends on:** TASK-04 (`f1572a76`), TASK-05 (`a63156e3`)
> **Date:** 2026-10-29
> **Note:** implemented together with TASK-09 at the operator's request; both
> contracts were written first, then each test file was run and fixed in turn.

## 1. User journey (from plan)

> As a user, I press `^S` to start, `^D` to stop/erase, `^K` to close, `^R` to
> hash-check, `+`/`-` for priority, `I` to toggle ignore, matching the classic
> rTorrent workflow.

| # | Acceptance criterion | Status |
|---|----------------------|--------|
| 1 | Shortcut dispatch produces typed actions executed against the bridge | PASS |
| 2 | `^S` → `TorrentAction::Start` | PASS |
| 3 | `^D` → active: `Stop`; stopped: `RemoveTorrent` | PASS |
| 4 | `^K` → `Execute("d.ignore_commands.set", 1)` + `Stop` + `Close` | PASS (as a batch) |
| 5 | `^R` → `Execute("d.complete.set", 0)` + `CheckHash` | PASS (as a batch) |
| 6 | `+`/`-` → priority cycling 0→1→2→3→0 | PASS |
| 7 | `I` → toggle `d.ignore_commands` via `Execute` | PASS (local view, see §9) |
| 8 | `Backspace` → load prompt, `^X` → command prompt | PASS (queued for TASK-11) |
| 9 | All actions require a focused torrent; no-op on empty list | PASS |

## 2. Validation commands (actually run)

| Gate | Command | Result |
|------|---------|--------|
| RED | `command cargo test -p rt-tui-client --test test_shortcuts` | FAIL (expected) |
| GREEN | `command cargo test -p rt-tui-client --test test_shortcuts` | PASS — 15 tests |
| Regression | full crate suite | PASS — 153 tests |
| Lint / format / check | `clippy --workspace --all-targets -D warnings`, `fmt --check`, `check --workspace --all-targets` | PASS |
| Coverage | `llvm-cov -p rt-tui-client` | 88.29 % lines (key_handler 98.44 %, app.rs 72.26 %) |
| Manual E2E | pty 95×20, `^S` | PASS (see §5) |

## 3. RED evidence

```text
error[E0432]: unresolved import `rt_tui_client::actions`
  --> crates/rt-tui-client/tests/test_shortcuts.rs:11:20

error[E0308]: mismatched types
  --> crates/rt-tui-client/tests/test_shortcuts.rs:89:30
   (handle_key returned `()`, the contract expects `Option<TuiAction>`)
```

Committed as RED checkpoint `64ffc4f2` (shared with TASK-09).

## 4. GREEN evidence

```text
Running tests/test_shortcuts.rs
test result: ok. 15 passed; 0 failed; 0 ignored
```

Three failures had to be fixed on the way to green:

1. `i_toggles_ignore_commands_between_on_and_off` — the local ignore flag was
   only updated when an action *executed*, so a second `handle_key` still saw the
   old value. Ignore state is now applied at dispatch time (see §8).
2. `control_action_is_executed_through_the_bridge` and
   `batch_actions_execute_every_step` — the shared mock daemon still answered
   `ControlTorrent`/`RemoveTorrent`/`SetTorrentPriority` with `Unimplemented`;
   those three RPCs are now implemented in `tests/common/mod.rs`.

Committed as `7449f9aa` (shared with TASK-09).

## 5. Manual end-to-end run

pty 95×20 against the mock daemon, `l` → `3` → `4` → `h` → `^S`:

```text
daemon log: CONTROL hash=abc123def456 action=1
```

`action=1` is `TORRENT_ACTION_START`, so the keystroke travelled
`handle_key → TuiAction → execute_action → GrpcBridge::control_torrent →
TorrentService.ControlTorrent`.

## 6. Test specification

| # | What is guaranteed | Test | Type | Result |
|---|--------------------|------|------|--------|
| 1 | `^S` yields a start action for the focused torrent | `test_shortcuts.rs:ctrl_s_produces_start_action` | unit | PASS |
| 2 | `^D` on an active torrent stops it | `ctrl_d_on_active_produces_stop` | unit | PASS |
| 3 | `^D` on a stopped torrent removes it | `ctrl_d_on_stopped_produces_remove` | unit | PASS |
| 4 | `+` advances priority 2 → 3 | `plus_cycles_priority_up` | unit | PASS |
| 5 | Shortcuts are no-ops without a torrent | `shortcuts_noop_on_empty_list` | unit | PASS |
| 6 | `-` wraps priority 0 → 3 | `minus_cycles_priority_down_and_wraps` | unit | PASS |
| 7 | `^K` batches ignore-set, stop and close | `ctrl_k_ignores_stops_and_closes` | unit | PASS |
| 8 | `^R` batches complete-reset and check-hash | `ctrl_r_resets_completion_and_checks_the_hash` | unit | PASS |
| 9 | `I` toggles ignore between 1 and 0 | `i_toggles_ignore_commands_between_on_and_off` | unit | PASS |
| 10 | Backspace/`^X` open the load/command prompts | `backspace_and_ctrl_x_open_prompts` | unit | PASS |
| 11 | `^Q` still quits and returns no action | `ctrl_q_still_quits_without_producing_an_action` | unit | PASS |
| 12 | A control action reaches `TorrentService` | `control_action_is_executed_through_the_bridge` | integration (in-process daemon) | PASS |
| 13 | Every step of a batch is executed in order | `batch_actions_execute_every_step` | integration | PASS |
| 14 | Prompt actions queue without a bridge | `prompt_actions_only_queue_a_request` | unit | PASS |
| 15 | A headless app cannot execute bridge actions | `headless_app_cannot_execute_bridge_actions` | unit | PASS |

## 7. Coverage and known gaps

```text
Filename           Regions  Missed  Cover   Lines  Missed  Cover
app.rs                 526     179  65.97%     328      91  72.26%
key_handler.rs          38       1  97.37%      64       1  98.44%
actions.rs               –       –       –       –       –       –   (pure enums)
TOTAL                 2359     336  85.76%    1588     186  88.29%
```

Known gaps:

- **Prompt actions only queue a request.** `take_prompt_request()` is consumed by
  nothing yet; TASK-11 renders the input prompt. The acceptance line is satisfied
  at the action level.
- **`d.ignore_commands` is a local view.** `TorrentSummary` does not report the
  flag, so the client remembers what it last sent (per hash, per session). A
  restart loses it until TASK-15/16 persist or re-query it.
- **Rejected commands surface only as `tracing::warn!`.** `TuiAction` execution
  returns the error to the loop but the UI does not show a message yet (TASK-15
  owns error surfacing); the bridge maps `success: false` onto
  `BridgeError::InvalidResponse` because the error enum is fixed at four variants.
- **`+`/`-` accept any modifiers** (`Char('+')` with or without Shift) since
  terminals disagree on shifted punctuation.
- **No confirmation for `^D`-removal** — rTorrent behaves the same way, and the
  plan does not ask for a prompt.
- `app.rs` coverage stays lower because `run`/`read_events` need a TTY.

## 8. REFACTOR notes

1. **Actions are data, dispatch is pure.** `key_handler::torrent_action` takes the
   focused torrent and the local ignore flag and returns `TuiAction`; every
   shortcut is testable without a daemon, and `App` only executes.
2. **Batches are flattened once** (`flatten`) before execution, so the executor
   has no recursive async call (which Rust cannot compile without boxing).
3. **Ignore state updates at dispatch time, not at execution time.** Learning this
   from the failing toggle test: the UI must reflect the user's intent even if the
   RPC is slow or the bridge is absent; execution is a separate concern.
4. **`execute_ints` on the bridge** keeps the int-only command path readable while
   `execute` remains the general entry point for TASK-11.
5. Rules review: no `unwrap()`/`expect()` in `src/`; `HashSet` for the ignore view;
   exhaustive `match` over `TuiAction` in the executor; no clones beyond the hash
   strings the actions must own.

## 9. Design decisions worth reviewing

- **`handle_key` now returns `Option<TuiAction>`** (the plan's contract). Existing
  callers ignore the value; the run loop executes it. This keeps the key map
  synchronous and the I/O in one place.
- **`^D` semantics follow the plan**: active → stop, stopped → remove, with no
  confirmation dialog.
- **`^K`/`^R` are `Batch` actions** so a partially failing sequence stops at the
  first error instead of continuing blindly.
- **Priority cycling uses `(p + 1) % 4`** and mirrors it downward, so `0 --` wraps
  to `3`.
- **Local ignore toggle**: sending `d.ignore_commands.set` without knowing the
  current value would be guesswork, so the client tracks what it sent.

## 10. Changed files

| Action | Path | Notes |
|--------|------|-------|
| CREATE | `crates/rt-tui-client/src/actions.rs` | `TuiAction`, `TorrentAction`, `PromptKind` (64 lines) |
| CREATE | `crates/rt-tui-client/src/key_handler.rs` | `torrent_action`, priority helpers (99 lines) |
| CREATE | `crates/rt-tui-client/tests/test_shortcuts.rs` | 15 tests (310 lines) |
| MODIFY | `crates/rt-tui-client/src/app.rs` | `handle_key`/`handle_event` return actions, `execute_action`, `flatten`, ignore view, prompt queue |
| MODIFY | `crates/rt-tui-client/src/bridge.rs` | `control_torrent`, `remove_torrent`, `set_torrent_priority`, `execute_ints`, `ensure_success` |
| MODIFY | `crates/rt-tui-client/src/state.rs` | `TorrentViewModel.is_active`/`priority` |
| MODIFY | `crates/rt-tui-client/src/lib.rs` | module wiring + re-exports |
| MODIFY | `crates/rt-tui-client/tests/common/mod.rs` | mock daemon implements control/remove/priority |

Commits: RED `64ffc4f2`, GREEN `7449f9aa`.

## 11. Not in scope (next plan task)

`TASK-11 — Input Prompt & Command Execution` consumes the queued
`PromptKind` and turns it into the real load/command prompt.
