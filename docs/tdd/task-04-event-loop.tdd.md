# TDD Evidence — TASK-04 — Event Loop & Terminal Backend

> **Source plan:** [`docs/tui-implementation.plan.md`](../tui-implementation.plan.md) — section *TASK-04 — Event Loop & Terminal Backend*
> **Skill:** `skills/tdd-workflow` (RED → GREEN → REFACTOR)
> **Rules applied:** `rules/rust/coding-style.md`, `rules/rust/patterns.md`
> **Depends on:** TASK-01 (`e175852a`), TASK-03 (`7aaaf3bd`)
> **Date:** 2026-10-27

## 1. User journey (from plan)

> As a user, I start `rstorrent-tui --endpoint localhost:5000` and see a TUI that
> enters raw mode, renders frames at the configured refresh rate, processes key
> events, and restores the terminal on `^Q`.

| # | Acceptance criterion | Status |
|---|----------------------|--------|
| 1 | `App` owns `AppState`, `GrpcBridge`, tick interval, terminal backend | PASS (`bridge` is `Option<GrpcBridge>` so headless/test mode has none; the terminal lives in `TerminalSession`, passed into `run`) |
| 2 | Loop: poll terminal events → dispatch to state → poll gRPC refresh → render | PASS |
| 3 | Raw mode + alternate screen on start, restore on exit/panic | PASS (RAII `Drop` + panic hook; verified end-to-end) |
| 4 | Bounded event queue with resize coalescing (`EventQueueConfig` pattern) | PASS |
| 5 | `^Q` sets `should_quit`; double `^Q` sets `force_quit` | PASS |
| 6 | Panic hook restores the terminal before aborting | PASS (installed in `main`, verified by inspection + manual run) |

## 2. Validation commands (actually run)

Remote builder still unreachable → local escape hatch (`command cargo ...`).

| Gate | Command | Result |
|------|---------|--------|
| RED | `command cargo test -p rt-tui-client --test test_event_loop` | FAIL (expected) |
| GREEN | `command cargo test -p rt-tui-client` | PASS — 70 tests (15 event-loop) |
| Lint | `command cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| Format | `command cargo fmt --check` | PASS |
| Compile gate | `command cargo check --workspace --all-targets` | PASS |
| Coverage | `command cargo llvm-cov -p rt-tui-client --summary-only --ignore-filename-regex 'main\.rs'` | 83.08 % lines |
| Manual E2E | mock daemon + `script` pty at 120×40, `^Q` | PASS (see §5) |

## 3. RED evidence

```text
error[E0432]: unresolved import `rt_tui_client::app`
  --> crates/rt-tui-client/tests/test_event_loop.rs:12:20
   |
12 | use rt_tui_client::app::App;
   |                    ^^^ could not find `app` in `rt_tui_client`

error[E0432]: unresolved import `rt_tui_client::events`
  --> crates/rt-tui-client/tests/test_event_loop.rs:14:20
   |
14 | use rt_tui_client::events::{AppEvent, EventPushOutcome, EventQueue, EventQueueConfig};
   |                    ^^^^^^ could not find `events` in `rt_tui_client`
```

Committed as RED checkpoint `4d674114`.

## 4. GREEN evidence

```text
Running tests/test_app_state.rs    test result: ok. 19 passed
Running tests/test_cli.rs          test result: ok. 5 passed
Running tests/test_endpoint.rs     test result: ok. 13 passed
Running tests/test_event_loop.rs   test result: ok. 15 passed
Running tests/test_grpc_bridge.rs  test result: ok. 15 passed
Running tests/test_run.rs          test result: ok. 3 passed
```

Committed as `f1572a76`.

## 5. Manual end-to-end verification

The plan's user journey cannot be asserted from a test process (it needs a real
TTY and a live daemon), so it was verified manually with a throwaway harness:

1. A temporary example started a `tonic` mock daemon on `127.0.0.1:50051`
   returning two torrents and session stats (file deleted afterwards — it is not
   part of the deliverable).
2. The binary ran under a pty: `script -qec "stty rows 40 cols 120; ./target/debug/rstorrent-tui --endpoint 127.0.0.1:50051 --refresh-ms 200"`.
3. `^Q` was piped in after two seconds.

Observed in the captured session:

```text
connected to 127.0.0.1:50051 (theme `default`, refresh 200 ms)   <- pre-flight
ESC[?1049h                                                       <- alternate screen entered (once)
ESC[1;3H rstorrent-tui  ESC[2;1H│ connected · 2 torrents │        <- frame content
… repeated frame resets …
ESC[?1049l ESC[?25h                                              <- alternate screen left, cursor shown (once)
COMMAND_EXIT_CODE="0"
```

So: raw mode + alternate screen entered exactly once, frames rendered at the
configured refresh, `^Q` produced a clean exit 0, and the terminal was restored.

## 6. Test specification

| # | What is guaranteed | Test | Type | Result |
|---|--------------------|------|------|--------|
| 1 | `^Q` sets `should_quit` | `test_event_loop.rs:app_quits_on_ctrl_q` | unit | PASS |
| 2 | A second `^Q` sets `force_quit` | `app_double_ctrl_q_sets_force_quit` | unit | PASS |
| 3 | Resize updates `terminal_cols`/`terminal_rows` | `app_resize_updates_state_dimensions` | unit | PASS |
| 4 | Plain `q` does not quit | `plain_q_does_not_quit` | unit | PASS |
| 5 | `^Q` followed by other keys stays a graceful quit | `ctrl_q_followed_by_other_keys_stays_a_graceful_quit` | unit | PASS |
| 6 | Key events dispatch through `handle_event` | `key_events_are_dispatched_through_handle_event` | unit | PASS |
| 7 | Resize events dispatch through `handle_event` | `resize_events_are_dispatched_through_handle_event` | unit | PASS |
| 8 | Three resizes collapse into the newest one | `event_queue_coalesces_resizes_to_the_latest` | unit | PASS |
| 9 | A full queue drops new events and counts them | `event_queue_is_bounded_and_drops_new_events_when_full` | unit | PASS |
| 10 | Zero capacity drops everything | `event_queue_with_zero_capacity_drops_everything` | unit | PASS |
| 11 | Refresh is due immediately, then only after the tick | `refresh_is_due_before_the_first_fetch_and_after_the_tick` | unit | PASS |
| 12 | A frame renders the title and the connection status | `draw_renders_title_and_connection_status` | unit (`TestBackend`) | PASS |
| 13 | A connected frame reports the torrent count | `draw_reports_torrent_count_when_connected` | unit (`TestBackend`) | PASS |
| 14 | Refresh drives `AppState` through the real bridge | `refresh_updates_state_from_the_daemon` | integration (in-process daemon) | PASS |
| 15 | A headless app has no bridge to refresh from | `headless_app_has_no_bridge_to_refresh_from` | unit | PASS |

## 7. Coverage and known gaps

```text
Filename       Regions  Missed  Cover   Functions  Missed  Executed   Lines  Missed  Cover
app.rs             182      89  51.10%         19       8   57.89%     119      50  57.98%
bridge.rs          152       6  96.05%         19       1   94.74%     108       2  98.15%
endpoint.rs         94       4  95.74%          9       0  100.00%      60       2  96.67%
events.rs           60       0 100.00%         12       0  100.00%      56       0 100.00%
run.rs              27       0 100.00%          3       0  100.00%      24       0 100.00%
state.rs           154       4  97.40%         19       1   94.74%     122       3  97.54%
terminal.rs         40      40   0.00%          6       6    0.00%      31      31   0.00%
TOTAL              709     143  79.83%         87      16   81.61%     520      88  83.08%
```

Known gaps (all TTY-bound):

- **`terminal.rs` is 0 %** — `enable_raw_mode`, `EnterAlternateScreen` and the
  panic hook cannot be exercised without a real terminal; a test that did so
  would flip the developer's own screen. Covered by the manual pty run above.
- **`app.rs` is 58 %** — `run`, `read_events` and `poll_input` block on crossterm
  polling, which needs a TTY. The pure parts (key/resize dispatch, scheduling,
  drawing, refresh) are covered.
- **`src/main.rs` is excluded** (composition root, as in TASK-01…03).
- **Resize events are not applied to a live terminal in a test** — `handle_resize`
  and the coalescing queue are unit-tested, the wiring is exercised only by the
  manual run.
- Only the affected crate is tested locally; full `cargo test --workspace` stays
  a CI/builder gate.

## 8. REFACTOR notes

1. **Blocking input reading moved off the async workers.** crossterm's
   `event::poll` blocks; the rules forbid blocking on the async runtime, so each
   iteration reads input inside `tokio::task::spawn_blocking` and returns a plain
   `Vec<AppEvent>`. The queue and its coalescing still happen inside that
   blocking section, which is exactly where resize floods occur.
2. **`handle_key` simplified to `if let`.** The initial single-arm `match` tripped
   `clippy::single_match`; the binding map grows in TASK-10, at which point it
   becomes a `match` again.
3. **No dependency on `rt-tui`.** The plan says "reuse the `EventQueueConfig`
   pattern"; importing `rt-tui` would pull `rt-core` back into the client, which
   is the coupling this crate exists to remove. `events.rs` mirrors the pattern
   (bounded capacity, `Coalesce`/`KeepAll`, `Enqueued`/`Coalesced`/`Dropped`,
   counters) with its own `AppEvent` type.
4. **Composition stays in `main.rs`**: pre-flight → real channel → terminal
   session → `App::run` → restore, with the terminal dropped before any error is
   printed so messages are visible.
5. Rules review: no `unwrap()`/`expect()` in `src/`, RAII instead of manual
   cleanup on every exit path, `EventQueue` allocation is bounded up front.

## 9. Design decisions worth reviewing

- **`bridge: Option<GrpcBridge>`** — the plan's tests require a synchronous
  `App::new_headless`, which cannot await a channel. The real binary always
  passes `Some`; headless mode is the explicit "no daemon" case.
- **`^Q` semantics** — the first press asks for a graceful exit (final frame is
  drawn), the second forces an immediate return. There is no time window; the
  plan only distinguishes the two presses.
- **`MAX_INPUT_WAIT = 50 ms`** — caps input latency while keeping refreshes on
  schedule regardless of `--refresh-ms`.
- **Refresh is marked before the RPC completes**, so a failing daemon cannot
  spin the loop; the error surfaces through `ConnectionStatus::Disconnected` in
  the frame instead.
- **Placeholder frame** — TASK-05/06 replace the block + status paragraph with
  real widgets; `draw` already takes `&Frame`, so no signature churn later.

## 10. Changed files

| Action | Path | Notes |
|--------|------|-------|
| CREATE | `crates/rt-tui-client/src/app.rs` | `App`, loop, dispatch, draw (198 lines) |
| CREATE | `crates/rt-tui-client/src/events.rs` | `AppEvent`, `EventQueue`, `EventQueueConfig` (156 lines) |
| CREATE | `crates/rt-tui-client/src/terminal.rs` | `TerminalSession`, `install_panic_hook` (74 lines) |
| CREATE | `crates/rt-tui-client/tests/test_event_loop.rs` | 15 tests (226 lines) |
| MODIFY | `crates/rt-tui-client/src/state.rs` | `terminal_cols`/`terminal_rows` |
| MODIFY | `crates/rt-tui-client/src/lib.rs` | new modules + re-exports |
| MODIFY | `crates/rt-tui-client/src/main.rs` | wires pre-flight → bridge → TUI |

Commits:

| Commit | Stage |
|--------|-------|
| `4d674114` | `test:` RED checkpoint |
| `f1572a76` | `feat:` GREEN + refactor |

## 11. Not in scope (next plan task)

`TASK-05 — Download List Widget` replaces the placeholder paragraph with the
torrent table driven by `AppState.torrents` and `focus_index`.
