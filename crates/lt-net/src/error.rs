use thiserror::Error;

#[derive(Debug, Error)]
pub enum NetError {
    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Input error: {0}")]
    Input(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Address parse error: {0}")]
    AddrParse(#[from] std::net::AddrParseError),
}
