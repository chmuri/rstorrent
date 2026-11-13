//! Typed errors for the session persistence layer.
//!
//! The orchestration layer (`SessionManager`, restore helpers) keeps
//! `io::Result` as its stable boundary; [`SessionError`] converts into
//! `io::Error` through [`SessionError::kind`] so existing `?` chains and
//! callers that match on `ErrorKind` keep working unchanged.

use std::io;

use lt_common::object::BencodeError;
use lt_storage::StorageError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error(transparent)]
    Io(#[from] io::Error),

    #[error("invalid bencode: {0}")]
    Bencode(#[from] BencodeError),

    #[error("invalid resume data: {0}")]
    Resume(#[from] StorageError),

    #[error("invalid session path: {0}")]
    InvalidPath(String),

    #[error("invalid info hash: {0}")]
    InvalidInfoHash(String),

    #[error("session destination is a directory: {0}")]
    DestinationIsDirectory(String),
}

impl SessionError {
    /// Maps the domain error back to the `io::ErrorKind` the persistence layer
    /// historically reported, so callers asserting on `kind()` stay valid.
    pub fn kind(&self) -> io::ErrorKind {
        match self {
            SessionError::Io(err) => err.kind(),
            SessionError::Bencode(_) | SessionError::Resume(_) => io::ErrorKind::InvalidData,
            SessionError::InvalidPath(_) | SessionError::InvalidInfoHash(_) => {
                io::ErrorKind::InvalidInput
            }
            SessionError::DestinationIsDirectory(_) => io::ErrorKind::Other,
        }
    }
}

impl From<SessionError> for io::Error {
    fn from(err: SessionError) -> io::Error {
        match err {
            SessionError::Io(err) => err,
            other => io::Error::new(other.kind(), other.to_string()),
        }
    }
}
