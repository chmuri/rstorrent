//! Startup path for the `rstorrent-tui` binary.

use crate::cli::Cli;
use crate::endpoint::{Endpoint, EndpointError, ProbeError};

/// Exit code returned when the `--endpoint` argument cannot be parsed.
pub const EXIT_INVALID_ENDPOINT: u8 = 2;

/// Exit code returned when the daemon cannot be reached.
pub const EXIT_DAEMON_UNREACHABLE: u8 = 1;

/// Resolves the endpoint, probes the daemon and reports the outcome.
///
/// Returns `Ok(())` once the daemon accepted a connection. Every failure is
/// reported as a [`StartupError`] so that `main` only has to map it onto an
/// exit code.
pub async fn run(cli: Cli) -> Result<(), StartupError> {
    let Cli {
        endpoint: raw_endpoint,
        theme,
        refresh_ms,
    } = cli;

    let endpoint: Endpoint = match raw_endpoint.parse() {
        Ok(endpoint) => endpoint,
        Err(source) => {
            return Err(StartupError::InvalidEndpoint {
                raw: raw_endpoint,
                source,
            })
        }
    };

    if let Err(source) = endpoint.probe().await {
        return Err(StartupError::Unreachable { endpoint, source });
    }

    println!("connected to {endpoint} (theme `{theme}`, refresh {refresh_ms} ms)");
    Ok(())
}

/// Reasons `rstorrent-tui` could not reach the daemon.
#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    /// The `--endpoint` argument was not a valid endpoint.
    #[error("invalid endpoint `{raw}`: {source}")]
    InvalidEndpoint {
        /// The raw argument, echoed back for the user.
        raw: String,
        /// Why the argument could not be parsed.
        #[source]
        source: EndpointError,
    },
    /// The daemon did not accept a connection.
    #[error("cannot reach {endpoint}: {source}")]
    Unreachable {
        /// The endpoint the client tried to reach.
        endpoint: Endpoint,
        /// Why the connection attempt failed.
        #[source]
        source: ProbeError,
    },
}

impl StartupError {
    /// Exit code the process should terminate with.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::InvalidEndpoint { .. } => EXIT_INVALID_ENDPOINT,
            Self::Unreachable { .. } => EXIT_DAEMON_UNREACHABLE,
        }
    }
}
