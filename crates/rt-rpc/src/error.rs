use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum RpcError {
    #[error("Input error: {0}")]
    InputError(String),

    #[error("Internal error: {0}")]
    InternalError(String),

    #[error("Untrusted error: {0}")]
    UntrustedError(String),

    #[error("Command not found: \"{0}\"")]
    CommandNotFound(String),

    #[error("Object is wrong type or const: {0}")]
    WrongType(String),

    #[error("RPC error ({code}): {message}")]
    Custom { code: i32, message: String },
}

impl From<&str> for RpcError {
    fn from(s: &str) -> Self {
        RpcError::InputError(s.to_string())
    }
}

impl From<String> for RpcError {
    fn from(s: String) -> Self {
        RpcError::InputError(s)
    }
}

impl From<lt_common::object::BencodeError> for RpcError {
    fn from(e: lt_common::object::BencodeError) -> Self {
        RpcError::WrongType(e.to_string())
    }
}
