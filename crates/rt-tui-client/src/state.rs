//! Application state (view model) consumed by the rendering layer.
//!
//! The state is owned by the single-threaded TUI loop — it is deliberately not
//! wrapped in `Arc`/`Mutex`. Proto messages are mapped into display-ready values
//! here so widgets never format or divide by zero themselves.

use rt_grpc::{SessionStatsResponse, TorrentSummary};

use crate::bridge::{BridgeError, GrpcBridge};

/// Index of the focused row in the torrent list.
pub type FocusIndex = usize;

const KIB: f64 = 1024.0;
const MIB: f64 = KIB * 1024.0;
const GIB: f64 = MIB * 1024.0;
const TIB: f64 = GIB * 1024.0;

/// Scales a byte count into a binary unit, e.g. `(1.43, "MB")`.
fn scale(bytes: i64) -> (f64, &'static str) {
    let value = bytes.max(0) as f64;
    if value >= TIB {
        (value / TIB, "TB")
    } else if value >= GIB {
        (value / GIB, "GB")
    } else if value >= MIB {
        (value / MIB, "MB")
    } else {
        (value / KIB, "KB")
    }
}

/// `1.4 MB` — sizes and totals.
fn format_size(bytes: i64) -> String {
    let (value, unit) = scale(bytes);
    format!("{value:.1} {unit}")
}

/// `   1.4 MB` — rates, right-aligned to six columns so the list stays tidy.
fn format_rate(bytes_per_sec: i64) -> String {
    let (value, unit) = scale(bytes_per_sec);
    format!("{value:>6.1} {unit}")
}

/// One torrent as the UI needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct TorrentViewModel {
    /// Info-hash, used as the stable row key and for RPC targeting.
    pub hash: String,
    /// Display name of the torrent.
    pub name: String,
    /// Human-readable state, e.g. `downloading`.
    pub state_name: String,
    /// Total size in bytes.
    pub size_bytes: i64,
    /// `2.0 GB`
    pub size_display: String,
    /// Bytes already completed.
    pub completed_bytes: i64,
    /// Completion in percent, clamped to `0.0..=100.0`.
    pub progress_percent: f64,
    /// Down rate in bytes per second.
    pub down_rate: i64,
    /// `   1.4 MB`
    pub down_rate_display: String,
    /// Up rate in bytes per second.
    pub up_rate: i64,
    /// `   0.0 KB`
    pub up_rate_display: String,
    /// `5.0 MB`
    pub down_total_display: String,
    /// `1.0 KB`
    pub up_total_display: String,
    /// Share ratio (`uploaded / downloaded`), `0.0` when nothing was downloaded.
    pub ratio: f64,
    /// ` 2.000`
    pub ratio_display: String,
}

impl TorrentViewModel {
    /// Maps a proto summary into the view model.
    pub fn from_proto(summary: &TorrentSummary) -> Self {
        let progress_percent = if summary.size_bytes > 0 {
            (summary.completed_bytes as f64 / summary.size_bytes as f64 * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };

        let ratio = if summary.down_total_bytes > 0 {
            (summary.up_total_bytes as f64 / summary.down_total_bytes as f64).max(0.0)
        } else {
            0.0
        };

        Self {
            hash: summary.hash.clone(),
            name: summary.name.clone(),
            state_name: summary.state_name.clone(),
            size_bytes: summary.size_bytes,
            size_display: format_size(summary.size_bytes),
            completed_bytes: summary.completed_bytes,
            progress_percent,
            down_rate: summary.down_rate_bytes_per_sec,
            down_rate_display: format_rate(summary.down_rate_bytes_per_sec),
            up_rate: summary.up_rate_bytes_per_sec,
            up_rate_display: format_rate(summary.up_rate_bytes_per_sec),
            down_total_display: format_size(summary.down_total_bytes),
            up_total_display: format_size(summary.up_total_bytes),
            ratio,
            ratio_display: format!("{ratio:>6.3}"),
        }
    }
}

/// Session-wide transfer statistics for the status bar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionStats {
    /// Down rate in bytes per second.
    pub down_rate: i64,
    /// `   1.4 MB`
    pub down_rate_display: String,
    /// Up rate in bytes per second.
    pub up_rate: i64,
    /// `   2.9 KB`
    pub up_rate_display: String,
    /// `5.0 MB`
    pub down_total_display: String,
    /// `1.0 KB`
    pub up_total_display: String,
    /// Torrents currently transferring.
    pub active_torrents: u32,
    /// Torrents known to the session.
    pub total_torrents: u32,
    /// Port the daemon listens on.
    pub listen_port: u32,
    /// Down throttle in bytes per second, `0` means unlimited.
    pub throttle_down_limit: i64,
    /// Up throttle in bytes per second, `0` means unlimited.
    pub throttle_up_limit: i64,
}

impl SessionStats {
    /// Maps a proto stats response into the view model.
    pub fn from_proto(response: &SessionStatsResponse) -> Self {
        Self {
            down_rate: response.down_rate_bytes_per_sec,
            down_rate_display: format_rate(response.down_rate_bytes_per_sec),
            up_rate: response.up_rate_bytes_per_sec,
            up_rate_display: format_rate(response.up_rate_bytes_per_sec),
            down_total_display: format_size(response.down_total_bytes),
            up_total_display: format_size(response.up_total_bytes),
            active_torrents: response.active_torrents,
            total_torrents: response.total_torrents,
            listen_port: response.listen_port,
            throttle_down_limit: response.throttle_down_limit,
            throttle_up_limit: response.throttle_up_limit,
        }
    }
}

/// Which rTorrent-style view the list is showing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ActiveView {
    /// All torrents.
    #[default]
    Main,
    /// Started torrents.
    Started,
    /// Stopped torrents.
    Stopped,
    /// Completed torrents.
    Complete,
    /// Incomplete torrents.
    Incomplete,
    /// Torrents with transfer activity.
    Active,
}

impl ActiveView {
    /// Name the daemon expects in `ListTorrentsRequest.view`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Started => "started",
            Self::Stopped => "stopped",
            Self::Complete => "complete",
            Self::Incomplete => "incomplete",
            Self::Active => "active",
        }
    }
}

/// Whether the last refresh reached the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// The last refresh succeeded.
    Connected,
    /// The last refresh failed; `reason` is the bridge error.
    Disconnected {
        /// Why the daemon could not be reached.
        reason: String,
    },
}

impl ConnectionStatus {
    /// `true` while the daemon is reachable.
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected)
    }
}

/// Everything the widgets render.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Torrents in the current view, in daemon order.
    pub torrents: Vec<TorrentViewModel>,
    /// Session-wide statistics.
    pub session_stats: SessionStats,
    /// Reachability of the daemon.
    pub connection: ConnectionStatus,
    /// View the torrent list is showing.
    pub active_view: ActiveView,
    /// Focused row in [`AppState::torrents`].
    pub focus_index: FocusIndex,
    /// Terminal width in columns as last reported by a resize event.
    pub terminal_cols: u16,
    /// Terminal height in rows as last reported by a resize event.
    pub terminal_rows: u16,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    /// Empty state, waiting for the first successful refresh.
    pub fn new() -> Self {
        Self {
            torrents: Vec::new(),
            session_stats: SessionStats::default(),
            connection: ConnectionStatus::Disconnected {
                reason: "not connected yet".to_string(),
            },
            active_view: ActiveView::Main,
            focus_index: 0,
            terminal_cols: 80,
            terminal_rows: 24,
        }
    }

    /// Refreshes torrents and stats; a failure only flips the connection status.
    pub async fn refresh(&mut self, bridge: &mut GrpcBridge) -> Result<(), BridgeError> {
        match self.fetch(bridge).await {
            Ok(()) => {
                self.connection = ConnectionStatus::Connected;
                Ok(())
            }
            Err(error) => {
                self.connection = ConnectionStatus::Disconnected {
                    reason: error.to_string(),
                };
                Err(error)
            }
        }
    }

    /// Fetches both RPCs first, then swaps the state in one step.
    async fn fetch(&mut self, bridge: &mut GrpcBridge) -> Result<(), BridgeError> {
        // `as_str` borrows nothing from `self`, so no clone is needed here.
        let view = self.active_view.as_str();
        let torrents = bridge.list_torrents(view).await?;
        let stats = bridge.session_stats().await?;

        self.torrents = torrents.iter().map(TorrentViewModel::from_proto).collect();
        self.session_stats = SessionStats::from_proto(&stats);
        self.clamp_focus();

        Ok(())
    }

    /// Moves the focus one row down, stopping at the last torrent.
    pub fn focus_down(&mut self) {
        if let Some(last) = self.torrents.len().checked_sub(1) {
            self.focus_index = (self.focus_index + 1).min(last);
        }
    }

    /// Moves the focus one row up, stopping at the first torrent.
    pub fn focus_up(&mut self) {
        self.focus_index = self.focus_index.saturating_sub(1);
    }

    /// Moves the focus `page` rows down, stopping at the last torrent.
    pub fn focus_page_down(&mut self, page: usize) {
        if let Some(last) = self.torrents.len().checked_sub(1) {
            self.focus_index = self.focus_index.saturating_add(page).min(last);
        }
    }

    /// Moves the focus `page` rows up, stopping at the first torrent.
    pub fn focus_page_up(&mut self, page: usize) {
        self.focus_index = self.focus_index.saturating_sub(page);
    }

    /// Moves the focus to the first torrent.
    pub fn focus_home(&mut self) {
        self.focus_index = 0;
    }

    /// Moves the focus to the last torrent.
    pub fn focus_end(&mut self) {
        self.focus_index = self.torrents.len().saturating_sub(1);
    }

    /// The focused torrent, if the list is not empty.
    pub fn selected(&self) -> Option<&TorrentViewModel> {
        self.torrents.get(self.focus_index)
    }

    /// Keeps the focus inside the list after a refresh.
    fn clamp_focus(&mut self) {
        self.focus_index = self.focus_index.min(self.torrents.len().saturating_sub(1));
    }
}
