//! Next-Gen Ratatui widgets for rtorrent console interface.

pub mod header;
pub mod inspector;
pub mod modals;
pub mod piece_map;
pub mod progress;
pub mod sidebar;
pub mod sparkline;
pub mod status_bar;
pub mod storage_gauge;
pub mod torrent_table;
pub mod view_tabs;

pub use header::DashboardHeader;
pub use inspector::{InspectorDrawerWidget, InspectorTab};
pub use modals::*;
pub use piece_map::PieceMapWidget;
pub use progress::{format_fractional_bar, SmoothProgressBar};
pub use sidebar::ViewsSidebar;
pub use sparkline::{format_rate, format_size, RateHistory, RateSparklineWidget};
pub use status_bar::{FooterMode, StatusBarWidget};
pub use storage_gauge::StorageGaugeWidget;
pub use torrent_table::TorrentTableWidget;
pub use view_tabs::ViewTabsWidget;
