# Contributing to rstorrent

Thank you for your interest in contributing to `rstorrent`!

## Code Architecture

The codebase is organized as a Cargo workspace with 14 crates:
- `crates/lt-*`: Engine, network, storage, protocol, DHT, and tracker crates (`libtorrent` domain).
- `crates/rt-*`: RPC engine, SCGI server, core session manager, and TUI (`rtorrent` domain).
- `crates/rstorrent`: Top-level CLI binary and supervisor.

## Development Workflow

1. Fork and clone the repository:
   ```bash
   git clone https://github.com/chmuri/rstorrent.git
   cd rstorrent
   ```

2. Verify that existing tests pass:
   ```bash
   cargo test --workspace
   ```

3. Make your changes adhering to idiomatic Rust standards:
   - Ensure `cargo check --workspace` compiles cleanly with zero warnings.
   - Run `cargo fmt --check` before submitting.
   - Run `cargo clippy --workspace --all-targets` to verify lints.

4. Add tests for any new features or bug fixes. Every crate has integration tests in its `tests/` directory.

5. Open a Pull Request on GitHub.

## Licensing

By contributing to `rstorrent`, you agree that your contributions will be licensed under the **GNU General Public License v2.0 or later** ([GPL-2.0-or-later](LICENSE)).
