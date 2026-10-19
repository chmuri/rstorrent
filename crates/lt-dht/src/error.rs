use thiserror::Error;

#[derive(Debug, Error)]
pub enum DhtError {
    #[error("Internal DHT error: {0}")]
    Internal(String),

    #[error("Input error: {0}")]
    Input(String),

    #[error("Network error: {0}")]
    Net(#[from] lt_net::NetError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
