# TDD Evidence — TASK-15 — Reconnection & Connection Health

> **Source plan:** [`docs/tui-implementation.plan.md`](../tui-implementation.plan.md) — *TASK-15*
> **Depends on:** TASK-14 (`df5dd313`)
> **Date:** 2026-10-29

## 1. Acceptance

| # | Criterion | Status |
|---|-----------|--------|
| 1 | `ConnectionManager` handles Connected → Disconnected → Reconnecting | PASS |
| 2 | Exponential backoff 1→2→4→8→30 s | PASS |
| 3 | Stale data indicator while disconnected | PASS (`AppState.stale`) |
| 4 | Status bar shows the reconnection countdown | PASS (`reconnect_in`) |
| 5 | Successful reconnect triggers a full refresh | PASS (`check_health` → `refresh`) |
| 6 | Health check every 10 s | PASS (`HEALTH_CHECK_INTERVAL`, `health_due`) |

## 2. Gates

| Gate | Command | Result |
|------|---------|--------|
| RED | `command cargo test -p rt-tui-client --test test_reconnection` | FAIL — `unresolved import rt_tui_client::connection` |
| GREEN | same | PASS — 11 tests |
| Lint/format/check | workspace clippy `-D warnings`, `fmt --check`, `check --all-targets` | PASS |
| Coverage | `llvm-cov -p rt-tui-client` | 86.97 % total, `connection.rs` 91.67 % |

## 3. Tests

| # | Guarantee | Test | Result |
|---|-----------|------|--------|
| 1 | Backoff doubles: 1 s → 2 s → 4 s | `backoff_increases_exponentially` | PASS |
| 2 | Backoff caps at 30 s | `backoff_caps_at_max` | PASS |
| 3 | Success resets the backoff | `successful_reconnect_resets_backoff` | PASS |
| 4 | State machine transitions | `state_transitions` | PASS |
| 5 | Full ladder 1,2,4,8,16,30,30 | `backoff_sequence_is_one_two_four_eight_sixteen_thirty` | PASS |
| 6 | Failures are counted | `failures_are_counted` | PASS |
| 7 | Health checks are due every 10 s | `health_checks_run_every_ten_seconds` | PASS |
| 8 | A failed refresh → Reconnecting + countdown + stale | `failed_refresh_moves_the_state_to_reconnecting` | PASS |
| 9 | A successful refresh clears the reconnection state | `successful_refresh_clears_the_reconnection_state` | PASS |
| 10 | Health check reports failure while down (lazy reconnect) | `health_check_reports_failure_while_the_daemon_is_down` | PASS |
| 11 | Status bar renders "reconnecting in Ns" | `status_bar_shows_the_reconnection_countdown` | PASS |

## 4. Design notes and gaps

- **`check_health` doubles as the reconnector**: with no channel it reconnects from
  the stored endpoint, then refreshes, which is what "reconnect automatically and
  resume showing data" needs.
- **Two failure signals feed one state**: refresh errors and health-check errors
  both go through `record_failure`, so the countdown cannot disagree with the dot.
- **Gap:** the countdown is published but not decremented between attempts; it
  reflects the backoff of the last failure (the loop re-derives it on each try).
- **Gap:** `GetVersion` is used as the ping; a dedicated health RPC would be
  cheaper but does not exist in the proto.
- **Gap:** reconnection is driven by the 500 ms loop, so the first retry can be up
  to one tick late.

## 5. Changed files

`src/connection.rs` (new), `src/app.rs` (`with_endpoint`, `check_health`,
`record_failure`, run-loop health checks), `src/widgets/status_bar.rs` (countdown),
`src/state.rs` (`reconnect_in`, `stale`), `tests/test_reconnection.rs`.
