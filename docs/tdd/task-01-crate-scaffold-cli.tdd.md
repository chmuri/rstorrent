# TDD Evidence — TASK-01 — Crate Scaffold & CLI

> **Source plan:** [`docs/tui-implementation.plan.md`](../tui-implementation.plan.md) — section *TASK-01 — Crate Scaffold & CLI*
> **Skill:** `skills/tdd-workflow` (RED → GREEN → REFACTOR)
> **Rules applied:** `rules/rust/coding-style.md`, `rules/rust/patterns.md`
> **Date:** 2026-10-27

## 1. User journey (from plan)

> As a user, I run `rstorrent-tui --endpoint localhost:5000` and the binary
> parses CLI arguments, validates the endpoint, and exits cleanly with a
> "connected" or "connection refused" message.

Acceptance criteria from the plan:

| # | Criterion | Status |
|---|-----------|--------|
| 1 | New workspace member `crates/rt-tui-client` with `Cargo.toml` | PASS |
| 2 | Binary target `rstorrent-tui` produced | PASS |
| 3 | CLI parsed with `clap`: `--endpoint`, `--theme`, `--refresh-ms` | PASS |
| 4 | Entry point creates a tokio runtime and exits cleanly | PASS |
| 5 | `check --workspace --all-targets` passes | PASS |

## 2. Validation commands (actually run)

The remote builder (`root@10.0.2.7`) was unreachable during this run (ping 100 %
loss, SSH timeout), so every gate below was executed locally via the documented
escape hatch `command cargo ...` instead of `scripts/remote-ci.sh`.

| Gate | Command | Result |
|------|---------|--------|
| RED | `command cargo test -p rt-tui-client` | FAIL (expected) |
| GREEN / unit + integration | `command cargo test -p rt-tui-client` | PASS — 21 tests |
| Lint | `command cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| Format | `command cargo fmt --check` | PASS |
| Compile gate (plan criterion) | `command cargo check --workspace --all-targets` | PASS |
| Coverage | `command cargo llvm-cov -p rt-tui-client --summary-only --ignore-filename-regex 'main\.rs'` | 97.62 % lines |

## 3. RED evidence

Scaffold only (`Cargo.toml`, empty `src/lib.rs`, stub `src/main.rs`) plus the test
targets; no implementation.

```text
error[E0432]: unresolved import `rt_tui_client::cli`
  --> crates/rt-tui-client/tests/test_cli.rs:4:20
   |
 4 | use rt_tui_client::cli::Cli;
   |                    ^^^ could not find `cli` in `rt_tui_client`

error[E0432]: unresolved import `rt_tui_client::endpoint`
 --> crates/rt-tui-client/tests/test_endpoint.rs:6:20
   |
 6 | use rt_tui_client::endpoint::{Endpoint, EndpointError, ProbeError};
   |                    ^^^^^^^^ could not find `endpoint` in `rt_tui_client`

error: could not compile `rt-tui-client` (test "test_cli") due to 1 previous error
error: could not compile `rt-tui-client` (test "test_endpoint") due to 1 previous error
```

This is a **compile-time RED**: both test targets were compiled, and compilation
failed only because the modules under test (`cli::Cli`, `endpoint::Endpoint`,
`endpoint::EndpointError`, `endpoint::ProbeError`) did not exist yet — not
because of broken test setup, missing third-party dependencies or unrelated
regressions. RED was re-confirmed after adding one more contract test
(`endpoint_rejects_missing_host`).

## 4. GREEN evidence

```text
Running tests/test_cli.rs
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

Running tests/test_endpoint.rs
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

Running tests/test_run.rs
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

Manual journey walkthrough of the built binary:

```text
$ ./target/debug/rstorrent-tui --endpoint localhost:1
cannot reach localhost:1: connection refused          # exit 1

$ ./target/debug/rstorrent-tui --endpoint "http://x:1"
invalid endpoint `http://x:1`: unsupported endpoint scheme `http` (...)  # exit 2

$ ./target/debug/rstorrent-tui --endpoint "unix:///tmp/does-not-exist.sock"
cannot reach unix:///tmp/does-not-exist.sock: No such file or directory (os error 2)  # exit 1

$ ./target/debug/rstorrent-tui --endpoint 127.0.0.1:<listening-port> --theme dracula --refresh-ms 250
connected to 127.0.0.1:<listening-port> (theme `dracula`, refresh 250 ms)  # exit 0

$ ./target/debug/rstorrent-tui --endpoint unix://<listening-socket>
connected to unix://<listening-socket> (theme `default`, refresh 500 ms)   # exit 0

$ ./target/debug/rstorrent-tui
error: the following required arguments were not provided: --endpoint <ENDPOINT>  # exit 2
```

## 5. Test specification

| # | What is guaranteed | Test file / test | Type | Result |
|---|--------------------|------------------|------|--------|
| 1 | `--endpoint localhost:5000` yields the raw endpoint and the `500` ms default refresh | `tests/test_cli.rs:cli_parses_tcp_endpoint` | unit | PASS |
| 2 | `--endpoint unix:///tmp/rstorrent.sock` is accepted verbatim | `tests/test_cli.rs:cli_parses_unix_socket_endpoint` | unit | PASS |
| 3 | Missing `--endpoint` is a parse error | `tests/test_cli.rs:cli_rejects_missing_endpoint` | unit | PASS |
| 4 | `--theme` defaults to `default` | `tests/test_cli.rs:cli_defaults_theme` | unit | PASS |
| 5 | `--theme` / `--refresh-ms` overrides are honoured | `tests/test_cli.rs:cli_accepts_theme_and_refresh_overrides` | unit | PASS |
| 6 | `host:port` parses to `Endpoint::Tcp` | `tests/test_endpoint.rs:endpoint_parses_host_and_port` | unit | PASS |
| 7 | `tcp://host:port` parses to `Endpoint::Tcp` | `tests/test_endpoint.rs:endpoint_parses_tcp_scheme` | unit | PASS |
| 8 | `unix:///path` parses to `Endpoint::Unix` | `tests/test_endpoint.rs:endpoint_parses_unix_socket` | unit | PASS |
| 9 | Empty endpoint is rejected | `tests/test_endpoint.rs:endpoint_rejects_empty_string` | unit | PASS |
| 10 | Missing port is rejected | `tests/test_endpoint.rs:endpoint_rejects_missing_port` | unit | PASS |
| 11 | Missing host (`:5000`) is rejected | `tests/test_endpoint.rs:endpoint_rejects_missing_host` | unit | PASS |
| 12 | Unknown scheme is rejected | `tests/test_endpoint.rs:endpoint_rejects_unknown_scheme` | unit | PASS |
| 13 | Non-numeric port is rejected | `tests/test_endpoint.rs:endpoint_rejects_invalid_port` | unit | PASS |
| 14 | `unix://` without a path is rejected | `tests/test_endpoint.rs:endpoint_rejects_empty_unix_path` | unit | PASS |
| 15 | `Display` round-trips through `FromStr` for all three forms | `tests/test_endpoint.rs:endpoint_display_round_trips` | unit | PASS |
| 16 | A listening TCP socket probes as reachable | `tests/test_endpoint.rs:probe_succeeds_on_listening_tcp_socket` | integration (loopback) | PASS |
| 17 | A closed TCP port surfaces `ProbeError::Refused` | `tests/test_endpoint.rs:probe_reports_refused_on_closed_port` | integration (loopback) | PASS |
| 18 | A listening Unix socket probes as reachable | `tests/test_endpoint.rs:probe_succeeds_on_listening_unix_socket` | integration (fs + socket) | PASS |
| 19 | A reachable daemon makes `run` return `Ok` and print the connected banner | `tests/test_run.rs:run_reports_success_on_listening_endpoint` | integration | PASS |
| 20 | A malformed endpoint makes `run` return `InvalidEndpoint` with exit code 2 | `tests/test_run.rs:run_rejects_malformed_endpoint` | integration | PASS |
| 21 | An unreachable daemon makes `run` return `Unreachable` with exit code 1 | `tests/test_run.rs:run_reports_unreachable_daemon` | integration | PASS |

## 6. Coverage and known gaps

```text
Filename       Regions  Missed  Cover   Functions  Missed  Executed   Lines  Missed  Cover
endpoint.rs         94       4  95.74%          9       0   100.00%      60       2  96.67%
run.rs              27       0 100.00%          3       0   100.00%      24       0 100.00%
TOTAL              121       4  96.69%         12       0   100.00%      84       2  97.62%
```

The 80 % threshold is met on the library surface (97.62 % lines). Known,
intentional gaps:

- **`src/main.rs` is excluded from the coverage report.** It is a 19-line binary
  entry point (`#[tokio::main]` + `ExitCode` mapping) that integration tests
  cannot link against; the logic it delegates to lives in `src/run.rs`, which is
  covered at 100 %. This is why the refactor step moved the startup path out of
  `main.rs` instead of leaving it untested.
- **`ProbeError::Timeout` is not exercised.** Triggering it deterministically
  would require a black-hole address or a mocked clock; the 2 s timeout arm is
  2 uncovered lines in `endpoint.rs`.
- **`src/cli.rs` reports no executable lines** because the whole struct is
  `clap` derive output; its behaviour is covered by the five `test_cli` cases.
- A refused-connection test briefly binds an ephemeral loopback port and frees
  it before probing. It is deterministic on Linux/macOS in practice but is the
  only test in this task whose precondition depends on OS port allocation.
- Only the affected crate was tested (`-p rt-tui-client`); the full
  `cargo test --workspace` suite was not run locally and is expected to run on
  CI/the remote builder.

## 7. REFACTOR notes

Applied after GREEN, behaviour unchanged, tests re-run green:

1. Extracted the startup path from `main.rs` into `src/run.rs` (`run(Cli) ->
   Result<(), StartupError>`), turning an untestable binary entry point into
   three covered integration tests. `main` now only maps the error onto an exit
   code.
2. `StartupError::exit_code()` returns `u8` instead of `std::process::ExitCode`,
   which makes the exit-code policy assertable and keeps `ExitCode` confined to
   `main`.
3. Removed the unused `Clone` derive from `EndpointError`; dropped the unused
   `Clone`-era duplication in tests behind a `tcp(host, port)` helper.
4. Reviewed against `rules/rust/coding-style.md`: no `unwrap()`/`expect()` in
   production code, `Cli` destructured to avoid a `String` clone in the error
   path, borrowed `&str`/`&self` throughout, all errors typed with `thiserror`,
   every `match` exhaustive with no wildcard arms.

## 8. Changed files

| Action | Path | Notes |
|--------|------|-------|
| CREATE | `crates/rt-tui-client/Cargo.toml` | 20 lines, `[[bin]] name = "rstorrent-tui"` |
| CREATE | `crates/rt-tui-client/src/lib.rs` | module wiring + public re-exports |
| CREATE | `crates/rt-tui-client/src/cli.rs` | `clap` `Cli` (33 lines) |
| CREATE | `crates/rt-tui-client/src/endpoint.rs` | `Endpoint`, `EndpointError`, `ProbeError`, `probe()` (152 lines) |
| CREATE | `crates/rt-tui-client/src/run.rs` | startup path + `StartupError` (73 lines) |
| CREATE | `crates/rt-tui-client/src/main.rs` | tokio entry, `ExitCode` mapping (19 lines) |
| CREATE | `crates/rt-tui-client/tests/test_cli.rs` | 5 tests |
| CREATE | `crates/rt-tui-client/tests/test_endpoint.rs` | 13 tests |
| CREATE | `crates/rt-tui-client/tests/test_run.rs` | 3 tests |
| MODIFY | `Cargo.toml` | workspace member + `rt-tui-client` workspace dependency |
| MODIFY | `Cargo.lock` | `+13` lines for the new member |

No git commits were created: the working tree also carries the untracked plan
file, and the TDD checkpoint commits were not requested for this run. The
RED/GREEN/REFACTOR evidence above is the merge record.

## 9. Not in scope (next plan task)

`TASK-02 — gRPC Client Bridge` is the next step: `tonic` channel construction over
the endpoint resolved here (`Endpoint::Tcp` / `Endpoint::Unix`). `tonic`,
`ratatui`, `crossterm` and `tracing` are already declared in the new crate's
manifest but are not yet used by TASK-01 code.
