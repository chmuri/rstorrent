//! ratatui widgets rendered by the client.

pub mod details;
pub mod download_list;
pub mod status_bar;
pub mod title_bar;

pub use details::DetailsWidget;
pub use download_list::DownloadListWidget;
pub use status_bar::StatusBarWidget;
pub use title_bar::TitleBarWidget;
