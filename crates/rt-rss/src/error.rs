//! Error types for the RSS subsystem.
//!
//! [`FetchError`] and [`ParseError`] live here temporarily so the crate
//! compiles while `fetch.rs`/`parse.rs` are still stubs. They move to those
//! modules when implemented; re-exports must stay here so
//! `crate::error::FetchError` / `crate::error::ParseError` keep working.

use thiserror::Error;

/// Top-level error for RSS operations.
#[derive(Debug, Error)]
pub enum RssError {
    /// Entity kind (e.g. "feed", "rule") and its id.
    #[error("{0} not found: {1}")]
    NotFound(&'static str, u64),

    #[error("invalid feed URL: {0}")]
    InvalidUrl(String),

    #[error("invalid rule: {0}")]
    InvalidRule(String),

    #[error("feed already registered: {0}")]
    DuplicateFeed(String),

    #[error(transparent)]
    Fetch(#[from] FetchError),

    #[error(transparent)]
    Parse(#[from] ParseError),

    #[error("store error: {0}")]
    Store(String),

    #[error("item {0} already processed")]
    ItemAlreadyProcessed(u64),
}

/// Errors from fetching a feed document or torrent payload over HTTP.
#[derive(Debug, Error)]
pub enum FetchError {
    #[error("request timed out")]
    Timeout,

    #[error("unexpected HTTP status {0}")]
    Http(u16),

    #[error("response exceeds the configured size limit")]
    TooLarge,

    #[error("response is not a torrent file")]
    NotTorrent,

    #[error("I/O error: {0}")]
    Io(String),

    #[error("invalid URL: {0}")]
    InvalidUrl(String),
}

/// Errors from parsing a feed document into items.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("XML error: {0}")]
    Xml(String),

    #[error("unsupported feed format")]
    UnsupportedFormat,

    #[error("feed contains more items than the configured limit")]
    TooManyItems,
}
