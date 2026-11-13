//! Typed errors for the `lt-sys` system bindings.

use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SysError {
    #[error("Cannot add notification event for empty paths.")]
    EmptyWatchPath,

    #[error("Could not resolve watch directory '{path:?}': {source}")]
    WatchResolve {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error(
        "Conflicting directory watch modes for watch directories: {existing:?} : {requested:?}"
    )]
    WatchConflict {
        existing: PathBuf,
        requested: PathBuf,
    },

    #[error("Scheduler::wait_until called on already scheduled entry.")]
    AlreadyScheduled,

    #[error("Scheduler entry is invalid.")]
    InvalidEntry,
}
