//! Crate `rt-tui-client`: standalone gRPC TUI client for rstorrent.
//!
//! The client renders an rTorrent-compatible interface with `ratatui` and talks
//! to a running `rstorrent` daemon over gRPC (`tonic`). Keeping the client out
//! of process with the daemon removes the `Arc<RwLock<..>>` coupling that the
//! in-process `rt-tui` crate still carries.

pub mod bridge;
pub mod cli;
pub mod endpoint;
pub mod run;

pub use bridge::{BridgeError, GrpcBridge, DEFAULT_RPC_TIMEOUT};
pub use cli::{Cli, DEFAULT_REFRESH_MS};
pub use endpoint::{Endpoint, EndpointError, ProbeError, PROBE_TIMEOUT};
pub use run::{run, StartupError, EXIT_DAEMON_UNREACHABLE, EXIT_INVALID_ENDPOINT};
