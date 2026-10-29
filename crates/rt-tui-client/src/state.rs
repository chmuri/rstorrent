//! Application state (view model) consumed by the rendering layer.
//!
//! The state is owned by the single-threaded TUI loop — it is deliberately not
//! wrapped in `Arc`/`Mutex`. Proto messages are mapped into display-ready values
//! here so widgets never format or divide by zero themselves.

use rt_grpc::{SessionStatsResponse, TorrentDetails, TorrentSummary};

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

/// Throttle limit, where `0` (or anything negative) means unlimited.
fn throttle_display(limit: i64) -> String {
    if limit <= 0 {
        "∞".to_string()
    } else {
        format_rate(limit)
    }
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// ` 100.0 KB`, or `∞` when unlimited.
    pub throttle_down_display: String,
    /// ` 100.0 KB`, or `∞` when unlimited.
    pub throttle_up_display: String,
}

impl Default for SessionStats {
    fn default() -> Self {
        Self::from_proto(&SessionStatsResponse::default())
    }
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
            throttle_down_display: throttle_display(response.throttle_down_limit),
            throttle_up_display: throttle_display(response.throttle_up_limit),
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

/// Tabs of the torrent details view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DetailTab {
    /// Summary of the torrent (default).
    #[default]
    Info,
    /// File list (TASK-08).
    Files,
    /// Connected peers (TASK-09).
    Peers,
    /// Trackers (TASK-09).
    Trackers,
    /// Piece map.
    Chunks,
}

impl DetailTab {
    /// All tabs in display order.
    pub const ALL: [Self; 5] = [
        Self::Info,
        Self::Files,
        Self::Peers,
        Self::Trackers,
        Self::Chunks,
    ];

    /// Label shown in the tab bar.
    pub fn title(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Files => "Files",
            Self::Peers => "Peers",
            Self::Trackers => "Trackers",
            Self::Chunks => "Chunks",
        }
    }

    /// One-based number shown in the tab bar and bound to the number keys.
    pub fn number(self) -> u8 {
        match self {
            Self::Info => 1,
            Self::Files => 2,
            Self::Peers => 3,
            Self::Trackers => 4,
            Self::Chunks => 5,
        }
    }

    /// Tab bound to `number`, if any.
    pub fn from_number(number: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|tab| tab.number() == number)
    }

    /// Next tab, wrapping after the last.
    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    /// Previous tab, wrapping before the first.
    pub fn previous(self) -> Self {
        let index = Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0);
        Self::ALL[(index + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// Info tab content for one torrent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetailsInfo {
    /// Info-hash of the torrent.
    pub hash: String,
    /// Display name.
    pub name: String,
    /// Human-readable state, e.g. `downloading`.
    pub state_name: String,
    /// `2.0 GB`
    pub size_display: String,
    /// `256.0 KB`
    pub piece_length_display: String,
    /// Number of pieces.
    pub piece_count: u32,
    /// Download directory, or `—` when the daemon does not report one.
    pub directory: String,
    /// Daemon message, or `—` when empty.
    pub message: String,
    /// Whether the torrent is marked private.
    pub private: bool,
    /// Seed connection description.
    pub connection_seed: String,
    /// Throttle group name.
    pub throttle_name: String,
}

/// Details pane: the active tab plus the data behind it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetailsState {
    /// Selected tab.
    pub active_tab: DetailTab,
    /// Info tab content.
    pub info: DetailsInfo,
    /// Number of files reported by the daemon.
    pub file_count: usize,
    /// Number of connected peers.
    pub peer_count: usize,
    /// Number of trackers.
    pub tracker_count: usize,
}

impl DetailsState {
    /// Maps a proto `TorrentDetails` into the details view model.
    pub fn from_proto(details: &TorrentDetails) -> Self {
        let summary = details.summary.as_ref();

        Self {
            active_tab: DetailTab::Info,
            info: DetailsInfo {
                hash: summary
                    .map(|summary| summary.hash.clone())
                    .unwrap_or_default(),
                name: summary
                    .map(|summary| summary.name.clone())
                    .unwrap_or_default(),
                state_name: summary
                    .map(|summary| summary.state_name.clone())
                    .unwrap_or_default(),
                size_display: format_size(summary.map_or(0, |summary| summary.size_bytes)),
                piece_length_display: format_size(i64::from(details.piece_length)),
                piece_count: details.num_pieces,
                directory: or_dash(summary.map(|summary| summary.directory.as_str())),
                message: or_dash(summary.map(|summary| summary.message.as_str())),
                private: details.is_private,
                connection_seed: details.connection_seed.clone(),
                throttle_name: details.throttle_name.clone(),
            },
            file_count: details.files.len(),
            peer_count: details.peers.len(),
            tracker_count: details.trackers.len(),
        }
    }

    /// Selects the next tab, wrapping after the last.
    pub fn next_tab(&mut self) {
        self.active_tab = self.active_tab.next();
    }

    /// Selects the previous tab, wrapping before the first.
    pub fn previous_tab(&mut self) {
        self.active_tab = self.active_tab.previous();
    }

    /// Selects a tab by its one-based number; `false` when out of range.
    pub fn select_tab(&mut self, number: u8) -> bool {
        match DetailTab::from_number(number) {
            Some(tab) => {
                self.active_tab = tab;
                true
            }
            None => false,
        }
    }
}

/// Falls back to an em dash for empty or missing daemon strings.
fn or_dash(value: Option<&str>) -> String {
    match value {
        Some(text) if !text.is_empty() => text.to_string(),
        _ => "—".to_string(),
    }
}

/// Whether the last refresh reached the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// The last refresh succeeded.
    Connected,
    /// A previous refresh failed and the client is retrying (TASK-15).
    Reconnecting,
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
    /// Open details pane; `None` while the torrent list is shown.
    pub details: Option<DetailsState>,
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
            details: None,
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
