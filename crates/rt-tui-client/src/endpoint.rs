//! Endpoint parsing and reachability probing for the gRPC daemon.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

/// Maximum time the client waits for the daemon to accept a connection.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

const TCP_SCHEME: &str = "tcp://";
const UNIX_SCHEME: &str = "unix://";

/// Where the `rstorrent` gRPC daemon listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// TCP endpoint, e.g. `localhost:5000` or `tcp://localhost:5000`.
    Tcp {
        /// Host name or IP address without the port suffix.
        host: String,
        /// TCP port the daemon listens on.
        port: u16,
    },
    /// Unix domain socket, e.g. `unix:///tmp/rstorrent.sock`.
    Unix {
        /// Filesystem path of the socket.
        path: PathBuf,
    },
}

impl Endpoint {
    /// Checks whether the daemon accepts a connection within [`PROBE_TIMEOUT`].
    pub async fn probe(&self) -> Result<(), ProbeError> {
        match tokio::time::timeout(PROBE_TIMEOUT, self.connect()).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(ProbeError::classify(error)),
            Err(_elapsed) => Err(ProbeError::Timeout(PROBE_TIMEOUT)),
        }
    }

    /// Opens and immediately drops a stream, or reports why that failed.
    async fn connect(&self) -> io::Result<()> {
        match self {
            Self::Tcp { host, port } => {
                drop(tokio::net::TcpStream::connect((host.as_str(), *port)).await?);
                Ok(())
            }
            Self::Unix { path } => {
                drop(tokio::net::UnixStream::connect(path).await?);
                Ok(())
            }
        }
    }
}

impl FromStr for Endpoint {
    type Err = EndpointError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let value = raw.trim();
        if value.is_empty() {
            return Err(EndpointError::Empty);
        }

        if let Some(path) = value.strip_prefix(UNIX_SCHEME) {
            if path.is_empty() {
                return Err(EndpointError::EmptyUnixPath);
            }
            return Ok(Self::Unix {
                path: PathBuf::from(path),
            });
        }

        let authority = value.strip_prefix(TCP_SCHEME).unwrap_or(value);
        if let Some((scheme, _rest)) = authority.split_once("://") {
            return Err(EndpointError::UnsupportedScheme(scheme.to_string()));
        }

        let (host, port) = authority
            .rsplit_once(':')
            .ok_or_else(|| EndpointError::MissingPort(value.to_string()))?;
        if host.is_empty() {
            return Err(EndpointError::MissingHost(value.to_string()));
        }
        let port = port
            .parse::<u16>()
            .map_err(|_| EndpointError::InvalidPort(value.to_string()))?;

        Ok(Self::Tcp {
            host: host.to_string(),
            port,
        })
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp { host, port } => write!(f, "{host}:{port}"),
            Self::Unix { path } => write!(f, "{UNIX_SCHEME}{}", path.display()),
        }
    }
}

/// Reasons an endpoint string cannot be turned into an [`Endpoint`].
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum EndpointError {
    /// The endpoint string was empty or whitespace only.
    #[error("endpoint is empty")]
    Empty,
    /// The endpoint used a scheme other than `tcp://` or `unix://`.
    #[error("unsupported endpoint scheme `{0}` (expected tcp://, unix:// or host:port)")]
    UnsupportedScheme(String),
    /// The endpoint had no `:port` suffix.
    #[error("endpoint `{0}` is missing a port")]
    MissingPort(String),
    /// The endpoint had no host before the `:port` suffix.
    #[error("endpoint `{0}` is missing a host")]
    MissingHost(String),
    /// The port part was not a valid `u16`.
    #[error("invalid port in endpoint `{0}`")]
    InvalidPort(String),
    /// The `unix://` scheme was given without a socket path.
    #[error("unix socket path is empty")]
    EmptyUnixPath,
}

/// Reasons a reachability probe failed.
#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    /// The daemon is not listening on the endpoint.
    #[error("connection refused")]
    Refused(#[source] io::Error),
    /// The daemon did not answer within the probe timeout.
    #[error("connection timed out after {0:?}")]
    Timeout(Duration),
    /// Any other I/O failure raised while connecting.
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl ProbeError {
    /// Separates a refused connection from every other I/O failure.
    fn classify(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::ConnectionRefused {
            Self::Refused(error)
        } else {
            Self::Io(error)
        }
    }
}
