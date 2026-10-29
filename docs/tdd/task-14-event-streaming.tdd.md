# TDD Evidence — TASK-14 — Event Streaming & Real-Time Updates

> **Source plan:** [`docs/tui-implementation.plan.md`](../tui-implementation.plan.md) — *TASK-14*
> **Skill:** `skills/tdd-workflow` · **Rules:** `rules/rust/coding-style.md`, `rules/rust/patterns.md`
> **Depends on:** TASK-02 (`de890203`), TASK-04 (`f1572a76`)
> **Date:** 2026-10-29

## 1. Acceptance

| # | Criterion | Status |
|---|-----------|--------|
| 1 | `EventSubscriber` wraps `TorrentService.StreamEvents` | PASS (`stream::Streams`, `GrpcBridge::stream_events`) |
| 2 | `StatsSubscriber` wraps `SystemService.StreamStats` | PASS (`GrpcBridge::stream_stats`) |
| 3 | Updates arrive on a bounded `mpsc` channel (256) | PASS (`STREAM_CAPACITY`) |
| 4 | `AppState::apply_event()` updates the list incrementally | PASS |
| 5 | `AppState::apply_stats()` updates session stats | PASS |
| 6 | Both streams reconnect with bounded backoff | PASS (`ConnectionManager`) |

## 2. Gates

| Gate | Command | Result |
|------|---------|--------|
| RED | `command cargo test -p rt-tui-client --test test_event_stream` | FAIL — `unresolved import rt_tui_client::stream` |
| GREEN | same | PASS — 10 tests |
| Lint/format/check | workspace clippy `-D warnings`, `fmt --check`, `check --all-targets` | PASS |
| Coverage | `llvm-cov -p rt-tui-client` | 86.97 % total, `stream.rs` 80.77 % |

## 3. Tests

| # | Guarantee | Test | Result |
|---|-----------|------|--------|
| 1 | A finished event marks the row complete | `apply_event_updates_torrent_state` | PASS |
| 2 | An unknown hash inserts a row | `apply_event_for_unknown_hash_adds_torrent` | PASS |
| 3 | Stats update rates | `apply_stats_updates_rates` | PASS |
| 4 | Repeated events replace instead of duplicating | `apply_event_replaces_an_existing_row` | PASS |
| 5 | `erased` events remove the row | `apply_event_removes_erased_torrents` | PASS |
| 6 | Events without a summary are ignored | `apply_event_ignores_events_without_summary` | PASS |
| 7 | The channel capacity is 256 | `stream_channel_is_bounded_to_256` | PASS |
| 8 | Torrent events reach the channel | `streams_forward_torrent_events` | PASS (live mock) |
| 9 | Stats reach the channel | `streams_forward_session_stats` | PASS (live mock) |
| 10 | `App::poll_streams` applies them to state | `app_applies_stream_updates` | PASS (live mock) |

## 4. Design notes and gaps

- **Two detached tasks, one channel**: each subscriber owns its own `GrpcBridge`
  and reconnects with the shared `ConnectionManager`; the `Streams` handle aborts
  both in `Drop`, so the tasks cannot outlive the UI.
- **Events are authoritative for their row**: `apply_event` rebuilds the row from
  the event summary rather than patching fields, so a partial summary cannot leave
  stale numbers behind.
- **Polling stays**: the periodic `refresh()` keeps working as a consistency
  fallback (and for daemons without streaming).
- **Gap:** the channel is bounded but a slow UI drops nothing — `mpsc::send` awaits,
  which applies backpressure to the stream instead of dropping events.
- **Gap:** event filters are not used (`event_filter` is always empty); the client
  applies every event.

## 5. Changed files

`src/stream.rs` (new), `src/bridge.rs` (`stream_events`, `stream_stats`),
`src/state.rs` (`apply_event`, `apply_stats`, `is_complete`), `src/app.rs`
(`poll_streams`, run-loop wiring), `tests/test_event_stream.rs`,
`tests/common/mod.rs` (streaming mock).
