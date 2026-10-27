//! Command-line interface for the `rstorrent-tui` binary.

use clap::Parser;

/// Default refresh interval for the UI loop, in milliseconds.
pub const DEFAULT_REFRESH_MS: u64 = 500;

/// Command-line arguments accepted by `rstorrent-tui`.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "rstorrent-tui",
    author,
    version,
    about = "rstorrent-tui - standalone terminal client for the rstorrent daemon",
    long_about = None
)]
pub struct Cli {
    /// Daemon endpoint: `host:port`, `tcp://host:port` or `unix:///path/to.sock`
    #[arg(long = "endpoint", value_name = "ENDPOINT")]
    pub endpoint: String,

    /// Colour theme name
    #[arg(long = "theme", value_name = "THEME", default_value = "default")]
    pub theme: String,

    /// Refresh interval for the UI loop, in milliseconds
    #[arg(
        long = "refresh-ms",
        value_name = "MILLISECONDS",
        default_value_t = DEFAULT_REFRESH_MS
    )]
    pub refresh_ms: u64,
}
