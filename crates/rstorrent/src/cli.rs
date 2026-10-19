use clap::Parser;

#[derive(Parser, Debug, Clone, Default)]
#[command(
    name = "rstorrent",
    author,
    version,
    about = "rstorrent - high-performance BitTorrent client written in Rust",
    long_about = None
)]
pub struct Cli {
    /// Bind address
    #[arg(short = 'b', value_name = "IP")]
    pub bind_address: Option<String>,

    /// Default directory
    #[arg(short = 'd', value_name = "DIRECTORY")]
    pub directory: Option<String>,

    /// Local address
    #[arg(short = 'i', value_name = "IP")]
    pub local_address: Option<String>,

    /// Port range
    #[arg(short = 'p', value_name = "PORT_RANGE")]
    pub port_range: Option<String>,

    /// Session directory
    #[arg(short = 's', value_name = "DIRECTORY")]
    pub session: Option<String>,

    /// Do not load config file (.rtorrent.rc)
    #[arg(short = 'n')]
    pub no_rc: bool,

    /// Specify config file location
    #[arg(short = 'c', value_name = "CONFIG_FILE")]
    pub config_file: Option<String>,

    /// Run as daemon without interactive TUI
    #[arg(long = "daemon")]
    pub daemon: bool,

    /// Port for gRPC server (e.g. 50051)
    #[arg(long = "grpc-port", value_name = "PORT")]
    pub grpc_port: Option<u16>,

    /// Unix socket path for gRPC server
    #[arg(long = "grpc-socket", value_name = "SOCKET_PATH")]
    pub grpc_socket: Option<String>,

    /// Set arbitrary options (e.g. -o key=value)
    #[arg(short = 'o', value_name = "OPTION", action = clap::ArgAction::Append)]
    pub options: Vec<String>,

    /// Execute command
    #[arg(short = 'O', value_name = "COMMAND", action = clap::ArgAction::Append)]
    pub command_options: Vec<String>,

    /// Enable deprecated commands
    #[arg(short = 'D')]
    pub deprecated: bool,

    /// Disable intermediate commands
    #[arg(short = 'I')]
    pub disable_intermediate: bool,

    /// Allow intermediate commands without XML-RPC
    #[arg(short = 'K')]
    pub intermediate_no_xmlrpc: bool,

    /// Torrent files, magnet links, or URLs to download
    #[arg(value_name = "FILES")]
    pub files: Vec<String>,
}

impl Cli {
    pub fn ignore_rc_file(&self) -> bool {
        self.no_rc
    }

    pub fn port(&self) -> Option<u16> {
        self.port_range.as_ref().and_then(|r| {
            if let Some((first, _)) = r.split_once('-') {
                first.parse().ok()
            } else {
                r.parse().ok()
            }
        })
    }
}
