# GitHub Copilot Code Review & Autofix Instructions

This document provides guidelines for GitHub Copilot when reviewing Pull Requests and diagnosing/fixing CI failures in the `rstorrent` repository.

## General Repository Guidelines

`rstorrent` is a memory-safe, high-performance BitTorrent client and engine written in pure Rust (edition 2021, MSRV 1.99+), organized as a multi-crate Cargo workspace.

### Key Architectural Principles
1. **Memory Safety & Correctness**: Ensure zero unsafe code blocks unless explicitly audited and documented in low-level primitives (`lt-sys`). Avoid unhandled `.unwrap()` in production paths; prefer idiomatic `Result`, `Option`, or structured `thiserror` error types.
2. **Concurrency & Async I/O**: The runtime is built on Tokio. Avoid holding blocking locks (such as standard `std::sync::Mutex` or `parking_lot::Mutex`) across `.await` points to prevent deadlocks and executor starvation.
3. **Protocol Standards**: Verify compliance with BitTorrent Enhancement Proposals (BEP 3, 6, 9, 10, 11, 15, 16) and SCGI/XML-RPC/JSON-RPC/gRPC wire protocols.
4. **Code Quality**: Code must pass `cargo check --workspace --all-targets`, `cargo clippy`, and adhere to `cargo fmt` formatting.

---

## Guidelines for Dependabot Pull Requests

When reviewing Pull Requests created by `@dependabot` (e.g. branch `dependabot/*`):

1. **Evaluate SemVer & Breaking Changes**:
   - Inspect the release notes and commit comparison provided in the PR description.
   - For `0.x` crate updates (e.g., `0.10 -> 0.11` or `0.29 -> 0.30`), verify whether any deprecated or breaking API changes affect our workspace usages.
2. **Review Verification**:
   - Confirm that all continuous integration checks (`CI/test (ubuntu-latest)`, `CI/test (macos-latest)`, `CI/rustfmt`, and `Dependency Review`) pass cleanly.
3. **Review Action & Comment**:
   - If the dependency update is clean, safe, and all CI tests pass:
     - Provide a brief summary of what the new version introduces (e.g., bug fixes, performance improvements, or security patches).
     - Submit an **Approve** review so that automated merge pipelines can proceed.
   - If the update causes a compilation failure, test regression, or introduces high-severity advisory alerts:
     - Request changes with a clear explanation of the failure.

---

## Automated CI Failure Diagnosis & Self-Healing Remediation

When requested to resolve CI failures or when triggered via **"Fix with Copilot"**:

### 1. Diagnosis Phase
- Extract the exact compiler diagnostics from the failed run (`error[E...]`, `mismatched types`, `unresolved import`, etc.).
- Identify which crate and source files triggered the compilation error.
- Check recent changes in `Cargo.lock` / `Cargo.toml` to determine if a dependency bump introduced breaking API changes (e.g., `getrandom::getrandom` -> `getrandom::fill`, `nix::fcntl::open` returning `OwnedFd` instead of `RawFd`, or renamed resolver builders).

### 2. Planning Phase
- Formulate a precise, minimal patch addressing the breaking change across all affected crates in the workspace.
- Avoid cascading architectural rewrites unless strictly necessary for type or lifetime compliance.
- Prefer updating call sites to match modern idiomatic APIs over pinning old deprecated methods.

### 3. Implementation Phase
- Ensure memory safety: Do not introduce `unsafe` blocks.
- Preserve existing comments and docstrings.
- Ensure all modified targets pass formatting (`cargo fmt --check`) and workspace checks (`cargo check --workspace --all-targets`).
- Verify that unit and integration tests compile and execute cleanly (`cargo test --workspace`).
