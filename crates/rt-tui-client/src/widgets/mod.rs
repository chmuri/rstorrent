//! ratatui widgets rendered by the client.

pub mod details;
pub mod download_list;
pub mod file_list;
pub mod input;
pub mod menu;
pub mod peer_list;
pub mod status_bar;
pub mod title_bar;
pub mod tracker_list;

pub use details::DetailsWidget;
pub use download_list::DownloadListWidget;
pub use file_list::FileListWidget;
pub use input::InputWidget;
pub use menu::MenuWidget;
pub use peer_list::PeerListWidget;
pub use status_bar::StatusBarWidget;
pub use title_bar::TitleBarWidget;
pub use tracker_list::TrackerListWidget;
