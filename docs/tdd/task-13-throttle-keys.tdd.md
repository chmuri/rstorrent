# TDD Evidence — TASK-13 — Throttle Controls & Keyboard Layouts

> **Source plan:** [`docs/tui-implementation.plan.md`](../tui-implementation.plan.md) — section *TASK-13 — Throttle Controls & Keyboard Layouts*
> **Skill:** `skills/tdd-workflow` (RED → GREEN → REFACTOR)
> **Rules applied:** `rules/rust/coding-style.md`, `rules/rust/patterns.md`
> **Depends on:** TASK-10 (`7449f9aa`)
> **Date:** 2026-10-29
> **Note:** implemented together with TASK-11 and TASK-12; contracts written first,
> then each test file run and fixed in turn.

## 1. User journey (from plan)

> As a user with `keys.layout=qwerty`, I press `a`/`z` for small upload throttle
> changes, `s`/`x` for medium, `d`/`c` for large. Uppercase letters control
> download throttle. With `keys.layout=dvorak`, the keys remap accordingly.

| # | Acceptance criterion | Status |
|---|----------------------|--------|
| 1 | `ThrottleKeyMap` maps layout → keys × (small, medium, large) | PASS |
| 2 | Layouts: `qwerty`, `azerty`, `qwertz`, `dvorak` | PASS (unknown → qwerty) |
| 3 | Step sizes default 1 KiB / 5 KiB / 50 KiB | PASS |
| 4 | Adjustments saturate at 0 | PASS (and at `u64::MAX`) |
| 5 | Changes dispatched via `throttle.global_up.max_rate.set` | PASS (`..._down...` for download) |

## 2. Validation commands (actually run)

| Gate | Command | Result |
|------|---------|--------|
| RED | `command cargo test -p rt-tui-client --test test_throttle` | FAIL (expected) |
| GREEN | same command | PASS — 16 tests |
| Regression | all 15 test files, one at a time | PASS — 202 tests |
| Lint / format / check | `clippy --workspace --all-targets -D warnings`, `fmt --check`, `check --workspace --all-targets` | PASS |
| Coverage | `llvm-cov -p rt-tui-client` | 88.09 % lines (throttle.rs 100 %) |
| Manual E2E | pty 95×20: `a` with a 10 000 B/s uplink | PASS (see §5) |

## 3. RED evidence

```text
error[E0432]: unresolved import `rt_tui_client::throttle`
  --> crates/rt-tui-client/tests/test_throttle.rs:10:20
```

Committed as RED checkpoint `edc7f093`.

## 4. GREEN evidence

```text
Running tests/test_throttle.rs
test result: ok. 16 passed; 0 failed; 0 ignored
```

One failure on the way to green was a **contract bug of mine**: the test named
`uppercase_a_lowers_the_download_limit` expected subtraction, while the same
file's plan-mandated `qwerty_layout_maps_uppercase_a_to_download_small_up`
requires uppercase to *raise*. Renamed to
`uppercase_a_raises_the_download_limit` and added
`uppercase_z_lowers_the_download_limit` to cover the decrease path.

Committed as `08dabb20`.

## 5. Manual end-to-end run

pty 95×20, mock daemon reporting `throttle_up_limit = 10 000`, key `a`:

```text
daemon log: EXECUTE throttle.global_up.max_rate.set args=[IntValue(11024)] target=
```

10 000 + 1024 = 11 024, i.e. the current limit plus the small step.

## 6. Test specification

| # | What is guaranteed | Test | Result |
|---|--------------------|------|--------|
| 1 | qwerty `a` → upload up, small | `qwerty_layout_maps_a_to_upload_small_up` | PASS |
| 2 | qwerty `A` → download up, small | `qwerty_layout_maps_uppercase_a_to_download_small_up` | PASS |
| 3 | dvorak `;` → upload down, small | `dvorak_layout_maps_semicolon_to_upload_small_down` | PASS |
| 4 | Subtracting below zero clamps | `throttle_adjustment_saturates_at_zero` | PASS |
| 5 | Unknown layouts behave like qwerty | `unknown_layout_falls_back_to_qwerty` | PASS |
| 6 | All six qwerty keys and their steps | `qwerty_covers_small_medium_large_in_both_directions` | PASS |
| 7 | azerty/qwertz remap the letter keys | `azerty_and_qwertz_remap_the_letter_keys` | PASS |
| 8 | Non-throttle keys return `None` | `unrelated_keys_are_not_throttle_keys` | PASS |
| 9 | Steps are 1/5/50 KiB | `step_sizes_are_kilobyte_based` | PASS |
| 10 | `adjust` moves up/down and saturates both ways | `adjust_moves_the_limit_and_saturates` | PASS |
| 11 | `a` builds an upload command with limit + step | `a_raises_the_upload_limit_through_a_command` | PASS |
| 12 | `A` raises the download limit | `uppercase_a_raises_the_download_limit` | PASS |
| 13 | `Z` lowers the download limit | `uppercase_z_lowers_the_download_limit` | PASS |
| 14 | Lowering below zero commands 0 | `lowering_below_zero_saturates` | PASS |
| 15 | Throttle keys work with an empty list | `throttle_keys_work_without_a_focused_torrent` | PASS |
| 16 | `App::with_layout("dvorak")` remaps at runtime | `app_can_use_another_layout` | PASS |

## 7. Coverage and known gaps

```text
throttle.rs    86    0  100.00%      76    0  100.00%
TOTAL        3198  448   85.99%    2167  258   88.09%
```

Known gaps:

- **The layout is not read from configuration yet** — `App::with_layout` exists
  but `main` always uses `qwerty`; TASK-16 (`Configuration`) wires `keys.layout`.
- **No visual feedback after a throttle change**: the status bar shows the limits,
  but they only move on the next refresh (500 ms unless `--refresh-ms` differs).
- **Throttle state is global only**; per-torrent throttle groups are TASK-16.
- **`azerty`/`qwertz` mappings assume the standard PC layouts**; alternative
  variants (e.g. Belgian azerty) are not modelled.

## 8. REFACTOR notes

1. **Layout data is two `[char; 3]` arrays** rather than a map, so `lookup` is a
   linear scan over three entries with no allocation.
2. **`ThrottleAction::{command, step, is_increase}`** keep the App free of
   command strings and direction logic.
3. **`adjust` is an associated function**, so saturation is testable without an
   `App` (and the plan's saturating-subtraction example maps onto it).
4. **Throttle dispatch sits after torrent actions** in `handle_list_key`, so `^S`
   still starts a torrent while plain `s` changes the throttle.

## 9. Design decisions worth reviewing

- **Uppercase = download, lowercase = upload** in both directions, exactly as the
  plan's contract requires.
- **Saturation instead of wrapping** at zero and at `u64::MAX`: rTorrent refuses
  negative limits, and `u64::MAX` is the natural "unlimited" ceiling.
- **Layout names are matched as lowercase strings**; anything unknown silently
  falls back to qwerty rather than failing startup.

## 10. Changed files

| Action | Path | Notes |
|--------|------|-------|
| CREATE | `crates/rt-tui-client/src/throttle.rs` | 147 lines |
| CREATE | `crates/rt-tui-client/tests/test_throttle.rs` | 16 tests (255 lines) |
| MODIFY | `crates/rt-tui-client/src/key_handler.rs` | unchanged dispatch order, throttle handled by the App |
| MODIFY | `crates/rt-tui-client/src/app.rs` | `with_layout`, `throttle_action`, key fallback |

Commits: RED `edc7f093`, GREEN `08dabb20`.

## 11. Not in scope

Reading `keys.layout` from configuration and per-torrent throttle groups
(TASK-16).
