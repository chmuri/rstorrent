//! RSS support for rstorrent: periodically fetch torrent feeds, match new
//! items against user-defined rules, and hand matches to the download engine.
//! The subsystem is inert until feeds are registered; persistent state lives
//! in the store and refresh/download orchestration in the manager.

pub mod error;
pub mod fetch;
pub mod manager;
pub mod model;
pub mod parse;
pub mod rules;
pub mod store;

pub use error::{FetchError, ParseError, RssError};
pub use model::{
    DownloadAction, DownloadRecord, DownloadResult, DownloadStatus, EpisodeFilter, Feed, FeedId,
    FeedItem, FeedPatch, HistoryEntry, ItemFilter, MatchedItem, RefreshOutcome, RssConfig, RssRule,
    RuleId,
};
