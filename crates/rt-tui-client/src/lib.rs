//! Crate `rt-tui-client`: standalone gRPC TUI client for rstorrent.
//!
//! The client renders an rTorrent-compatible interface with `ratatui` and talks
//! to a running `rstorrent` daemon over gRPC (`tonic`). Keeping the client out
//! of process with the daemon removes the `Arc<RwLock<..>>` coupling that the
//! in-process `rt-tui` crate still carries.

pub mod app;
pub mod bridge;
pub mod cli;
pub mod endpoint;
pub mod events;
pub mod run;
pub mod state;
pub mod terminal;
pub mod widgets;

pub use app::{App, DEFAULT_TICK};
pub use bridge::{BridgeError, GrpcBridge, DEFAULT_RPC_TIMEOUT};
pub use cli::{Cli, DEFAULT_REFRESH_MS};
pub use endpoint::{Endpoint, EndpointError, ProbeError, PROBE_TIMEOUT};
pub use events::{AppEvent, EventPushOutcome, EventQueue, EventQueueConfig, ResizePolicy};
pub use run::{run, StartupError, EXIT_DAEMON_UNREACHABLE, EXIT_INVALID_ENDPOINT};
pub use state::{
    ActiveView, AppState, ConnectionStatus, DetailTab, DetailsInfo, DetailsState, FileEntry,
    FileListState, FocusIndex, SessionStats, TorrentViewModel,
};
pub use terminal::{install_panic_hook, ClientTerminal, TerminalSession};
pub use widgets::{
    DetailsWidget, DownloadListWidget, FileListWidget, StatusBarWidget, TitleBarWidget,
};
