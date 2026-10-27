# TDD Evidence — TASK-02 — gRPC Client Bridge

> **Source plan:** [`docs/tui-implementation.plan.md`](../tui-implementation.plan.md) — section *TASK-02 — gRPC Client Bridge*
> **Skill:** `skills/tdd-workflow` (RED → GREEN → REFACTOR)
> **Rules applied:** `rules/rust/coding-style.md`, `rules/rust/patterns.md`
> **Depends on:** TASK-01 (merged as `e175852a`)
> **Date:** 2026-10-27

## 1. User journey (from plan)

> As the TUI event loop, I call `GrpcBridge::list_torrents("main")` and receive
> a `Vec<TorrentSummary>` or a typed `BridgeError::ConnectionLost`.

Acceptance criteria from the plan:

| # | Criterion | Status |
|---|-----------|--------|
| 1 | `GrpcBridge` wraps tonic-generated clients for all 3 services | PASS |
| 2 | Connects to TCP (`host:port`) or Unix socket (`unix:///path`) | PASS |
| 3 | All methods return `Result<T, BridgeError>` — never panic | PASS |
| 4 | Bounded timeout on every RPC (configurable, default 5 s) | PASS |
| 5 | `BridgeError` distinguishes `ConnectionLost`, `Timeout`, `ServerError`, `InvalidResponse` | PASS |

## 2. Validation commands (actually run)

Remote builder still unreachable, so the local escape hatch was used again
(`command cargo ...` instead of `scripts/remote-ci.sh`).

| Gate | Command | Result |
|------|---------|--------|
| RED | `command cargo test -p rt-tui-client --test test_grpc_bridge` | FAIL (expected) |
| GREEN | `command cargo test -p rt-tui-client` | PASS — 36 tests (15 bridge) |
| Lint | `command cargo clippy --workspace --all-targets -- -D warnings` | PASS |
| Format | `command cargo fmt --check` | PASS |
| Compile gate | `command cargo check --workspace --all-targets` | PASS |
| Coverage | `command cargo llvm-cov -p rt-tui-client --summary-only --ignore-filename-regex 'main\.rs'` | 97.87 % lines |

## 3. RED evidence

Test file added against a crate that has no `bridge` module yet. The mock
daemon and the harness compile; the only error is the missing module:

```text
error[E0432]: unresolved import `rt_tui_client::bridge`
  --> crates/rt-tui-client/tests/test_grpc_bridge.rs:20:20
   |
20 | use rt_tui_client::bridge::{BridgeError, GrpcBridge};
   |                    ^^^^^^ could not find `bridge` in `rt_tui_client`

error: could not compile `rt-tui-client` (test "test_grpc_bridge") due to 1 previous error
```

Committed as the RED checkpoint `b07d3d16` (`test: add failing gRPC bridge contract`).

Harness fixes made **before** accepting RED (so RED is not a setup artefact):
`tonic 0.14` returns a `Router` from `Server::builder().add_service(..)`, and
`serve_with_incoming` is a method on that router, not on `Server`.

## 4. GREEN evidence

```text
Running tests/test_cli.rs          test result: ok. 5 passed
Running tests/test_endpoint.rs     test result: ok. 13 passed
Running tests/test_grpc_bridge.rs  test result: ok. 15 passed
Running tests/test_run.rs          test result: ok. 3 passed
```

Committed as `de890203` (`feat: add gRPC client bridge for the daemon`).

## 5. Test specification

| # | What is guaranteed | Test | Type | Result |
|---|--------------------|------|------|--------|
| 1 | An unreachable server yields `BridgeError::ConnectionLost` | `test_grpc_bridge.rs:bridge_returns_connection_error_when_server_unreachable` | integration (real TCP connect to closed port) | PASS |
| 2 | `BridgeError::ConnectionLost` Display contains the endpoint | `bridge_error_display_includes_endpoint` | unit | PASS |
| 3 | The four plan variants are matchable without a wildcard | `bridge_error_variants_are_exhaustive` | unit (compile-time) | PASS |
| 4 | `list_torrents` decodes summaries from the daemon | `list_torrents_returns_summaries` | integration (in-process tonic server) | PASS |
| 5 | `get_torrent` returns validated details | `get_torrent_returns_details` | integration | PASS |
| 6 | Details without a summary become `InvalidResponse` | `get_torrent_rejects_details_without_summary` | integration | PASS |
| 7 | `version()` reads the SystemService banner | `version_returns_server_version` | integration | PASS |
| 8 | An empty version string becomes `InvalidResponse` | `version_rejects_empty_version_string` | integration | PASS |
| 9 | `execute()` reaches CommandService and returns its response | `execute_returns_command_response` | integration | PASS |
| 10 | A gRPC error status becomes `ServerError { code, message }` | `server_error_is_mapped_to_server_error` | integration | PASS |
| 11 | A slow RPC becomes `Timeout` instead of hanging | `slow_rpc_is_reported_as_timeout` | integration (50 ms budget, 500 ms handler) | PASS |
| 12 | `tcp://host:port` is normalised into a working URI | `connects_with_tcp_scheme_prefix` | integration | PASS |
| 13 | A blank endpoint is a connection error, not a panic | `empty_endpoint_is_a_connection_error` | unit | PASS |
| 14 | `unix://` without a path is a connection error | `empty_unix_path_is_a_connection_error` | unit | PASS |
| 15 | The bridge talks to a real Unix domain socket | `connects_over_unix_socket` | integration (UDS + tonic server) | PASS |

The in-process daemon implements the generated `TorrentService`, `SystemService`
and `CommandService` traits in one `MockDaemon` type; unimplemented RPCs return
`Status::unimplemented`, streaming is left to TASK-14.

## 6. Coverage and known gaps

```text
Filename       Regions  Missed  Cover   Functions  Missed  Executed   Lines  Missed  Cover
bridge.rs          144       6  95.83%         17       1   94.12%     104       2  98.08%
endpoint.rs         94       4  95.74%          9       0  100.00%      60       2  96.67%
run.rs              27       0 100.00%          3       0  100.00%      24       0 100.00%
TOTAL              265      10  96.23%         29       1   96.55%     188       4  97.87%
```

Known gaps:

- **`https://` endpoint branch is not covered.** The daemon has no TLS listener
  in this task; the branch is a pass-through spelling of the `http://` one.
- **`src/main.rs` is excluded** (binary entry point, unchanged from TASK-01).
- **Streaming RPCs (`stream_events`, `stream_stats`) are not implemented** — the
  plan schedules them in TASK-14; the mocks return `unimplemented`.
- **`run()` still probes the endpoint with `Endpoint::probe()`** rather than
  opening a `GrpcBridge`. Wiring the bridge into the event loop belongs to
  TASK-04 (`Event Loop & Terminal Backend`), and TASK-01's
  `run_reports_success_on_listening_endpoint` test deliberately uses a plain TCP
  listener, which a gRPC handshake would reject.
- Only the affected crate is tested locally (`-p rt-tui-client`); the full
  `cargo test --workspace` remains a CI/builder gate.

## 7. REFACTOR notes

1. **Dropped the hand-written UDS connector.** tonic 0.14 gained native Unix
   socket support (`Endpoint::from_shared("unix:///…")` → `EndpointType::Uds`),
   so `tower::service_fn` + `connect_with_connector` + a `hyper-util` adapter
   were unnecessary. `tower` was declared for the RED checkpoint and removed
   again in the GREEN commit; the manifests ship without it.
2. **Removed the `bounded_connect` generic helper.** With one concrete transport
   there is a single call site, so the timeout match lives inline in
   `connect_channel` — less indirection, same behaviour.
3. `Target` enum collapsed into `transport_uri()`, a pure `&str -> String`
   normaliser that is trivially testable and allocation-free for the pass-through
   spellings.
4. Rules review: no `unwrap()`/`expect()` in `bridge.rs`; `Option::unwrap_or_default`
   used instead of `unwrap`; one `Channel::clone()` is required because three
   generated clients share a single multiplexed channel; every `match` on
   `BridgeError`-producing code is exhaustive.

## 8. Design decisions worth reviewing

- **Malformed endpoint strings map to `ConnectionLost`.** The plan's contract test
  matches `BridgeError` exhaustively over exactly four variants, so adding
  `InvalidEndpoint` would break it. The message keeps the cause
  (`` `{raw}`: {reason} ``) so nothing is lost at runtime.
- **Invalid-response validation is deliberate, not incidental.** `get_torrent`
  rejects details without a summary and `version()` rejects an empty version
  string; both are observable protocol violations and give `InvalidResponse` a
  real meaning.
- **Timeout is applied per call, not per channel only.** `connect()` and every
  RPC go through `tokio::time::timeout`, so a wedged daemon cannot hang the TUI.

## 9. Changed files

| Action | Path | Notes |
|--------|------|-------|
| CREATE | `crates/rt-tui-client/src/bridge.rs` | `GrpcBridge`, `BridgeError`, `transport_uri` (188 lines) |
| CREATE | `crates/rt-tui-client/tests/test_grpc_bridge.rs` | 15 tests + in-process mock daemon (443 lines) |
| MODIFY | `crates/rt-tui-client/src/lib.rs` | `pub mod bridge` + re-exports |
| MODIFY | `crates/rt-tui-client/Cargo.toml` | `rt-grpc` dependency, `tokio-stream` dev-dependency |
| MODIFY | `Cargo.lock` | new member dependencies |

Commits:

| Commit | Stage |
|--------|-------|
| `b07d3d16` | `test:` RED checkpoint (failing contract + harness) |
| `de890203` | `feat:` GREEN + refactor |

## 10. Not in scope (next plan task)

`TASK-03 — Application State (View Model)` builds `AppState` on top of this
bridge; TASK-04 wires both into the ratatui event loop that replaces
`Endpoint::probe()` in `run()`.
