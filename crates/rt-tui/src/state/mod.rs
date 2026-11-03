//! Modern, cache-friendly UI state structures for the next-generation rtorrent UI.

use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TorrentUiState {
    #[default]
    Downloading,
    Seeding,
    Active,
    Paused,
    Complete,
    Checking,
    Error,
}

impl TorrentUiState {
    pub fn badge(&self) -> (&'static str, &'static str) {
        match self {
            Self::Downloading => ("⬇", "DOWN"),
            Self::Seeding => ("⬆", "SEED"),
            Self::Active => ("⚡", "ACTV"),
            Self::Paused => ("⏸", "PAUS"),
            Self::Complete => ("✔", "DONE"),
            Self::Checking => ("⟳", "CHCK"),
            Self::Error => ("✖", "ERR "),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ActiveView {
    #[default]
    All = 1,
    Downloading = 2,
    Seeding = 3,
    Active = 4,
    Paused = 5,
    Complete = 6,
    Error = 7,
    Hashing = 8,
}

impl ActiveView {
    pub const ALL: [ActiveView; 8] = [
        ActiveView::All,
        ActiveView::Downloading,
        ActiveView::Seeding,
        ActiveView::Active,
        ActiveView::Paused,
        ActiveView::Complete,
        ActiveView::Error,
        ActiveView::Hashing,
    ];

    pub fn title(&self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Downloading => "Downloading",
            Self::Seeding => "Seeding",
            Self::Active => "Active",
            Self::Paused => "Paused",
            Self::Complete => "Complete",
            Self::Error => "Error",
            Self::Hashing => "Hashing",
        }
    }

    pub fn from_number(num: usize) -> Option<Self> {
        match num {
            1 => Some(Self::All),
            2 => Some(Self::Downloading),
            3 => Some(Self::Seeding),
            4 => Some(Self::Active),
            5 => Some(Self::Paused),
            6 => Some(Self::Complete),
            7 => Some(Self::Error),
            8 => Some(Self::Hashing),
            _ => None,
        }
    }

    pub fn next(&self) -> Self {
        match self {
            Self::All => Self::Downloading,
            Self::Downloading => Self::Seeding,
            Self::Seeding => Self::Active,
            Self::Active => Self::Paused,
            Self::Paused => Self::Complete,
            Self::Complete => Self::Error,
            Self::Error => Self::Hashing,
            Self::Hashing => Self::All,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            Self::All => Self::Hashing,
            Self::Downloading => Self::All,
            Self::Seeding => Self::Downloading,
            Self::Active => Self::Seeding,
            Self::Paused => Self::Active,
            Self::Complete => Self::Paused,
            Self::Error => Self::Complete,
            Self::Hashing => Self::Error,
        }
    }

    pub fn matches(&self, record: &TorrentUiRecord) -> bool {
        match self {
            Self::All => true,
            Self::Downloading => record.state == TorrentUiState::Downloading,
            Self::Seeding => record.state == TorrentUiState::Seeding,
            Self::Active => {
                record.down_rate_bps > 0
                    || record.up_rate_bps > 0
                    || record.state == TorrentUiState::Downloading
                    || record.state == TorrentUiState::Active
            }
            Self::Paused => record.state == TorrentUiState::Paused,
            Self::Complete => record.completed_bytes >= record.size_bytes && record.size_bytes > 0,
            Self::Error => record.state == TorrentUiState::Error,
            Self::Hashing => record.state == TorrentUiState::Checking,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    DownRate,
    UpRate,
    Name,
    Progress,
    Size,
    Ratio,
    Eta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    #[default]
    Descending,
    Ascending,
}

/// Status of an individual piece in the torrent
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PieceState {
    #[default]
    Missing,
    PeerAvailable,
    Downloading,
    Verified,
}

/// Flat, contiguous, highly cache-efficient record of a single torrent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentUiRecord {
    pub hash: [u8; 20],
    pub hash_hex: String,
    pub name: String,
    pub size_bytes: u64,
    pub completed_bytes: u64,
    pub down_rate_bps: u64,
    pub up_rate_bps: u64,
    pub total_uploaded_bytes: u64,
    pub state: TorrentUiState,
    pub seeds_connected: u32,
    pub seeds_total: u32,
    pub peers_connected: u32,
    pub peers_total: u32,
    pub priority: u8,
    pub eta_seconds: Option<u64>,
    pub ratio_promille: u32, // ratio * 1000
    pub tags: Vec<String>,
    pub piece_size: u64,
    pub total_pieces: u32,
    pub verified_pieces: u32,
    pub save_path: String,
    pub is_private: bool,
    pub message: Option<String>,
}

impl TorrentUiRecord {
    pub fn progress_fraction(&self) -> f64 {
        if self.size_bytes == 0 {
            0.0
        } else {
            (self.completed_bytes as f64 / self.size_bytes as f64).clamp(0.0, 1.0)
        }
    }

    pub fn progress_percent(&self) -> f64 {
        self.progress_fraction() * 100.0
    }

    pub fn ratio(&self) -> f64 {
        self.ratio_promille as f64 / 1000.0
    }
}

/// Swarm peer information for Inspector drawer
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerUiRecord {
    pub ip_port: String,
    pub client_name: String,
    pub flags: String, // e.g. "u/d/E"
    pub down_rate_bps: u64,
    pub up_rate_bps: u64,
    pub progress_percent: u8,
    pub is_encrypted: bool,
}

/// Tracker information for Inspector drawer
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerUiRecord {
    pub index: usize,
    pub tier: u32,
    pub url: String,
    pub status: String,
    pub seeds: u32,
    pub peers: u32,
    pub interval_secs: u32,
    pub next_announce_secs: u32,
}

/// File information for Inspector drawer
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileUiRecord {
    pub index: usize,
    pub path: String,
    pub size_bytes: u64,
    pub completed_bytes: u64,
    pub priority: u8, // 0 = Off, 1 = Normal, 2 = High
}

/// Detailed diagnostics of a selected torrent for the inspector drawer
#[derive(Debug, Clone, Default)]
pub struct TorrentDetailSnapshot {
    pub peers: Vec<PeerUiRecord>,
    pub trackers: Vec<TrackerUiRecord>,
    pub files: Vec<FileUiRecord>,
    pub piece_map: Vec<PieceState>,
    pub log_messages: Vec<String>,
}

/// Global session statistics
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionStats {
    pub global_down_bps: u64,
    pub global_up_bps: u64,
    pub global_down_total: u64,
    pub global_up_total: u64,
    pub disk_free_bytes: u64,
    pub disk_total_bytes: u64,
    pub dht_nodes: u32,
    pub listen_port: u16,
    pub port_open: bool,
    pub total_torrents: u32,
    pub active_torrents: u32,
}

/// Complete immutable snapshot of daemon state, published over `tokio::sync::watch`.
#[derive(Debug, Clone)]
pub struct EngineSnapshot {
    pub sequence: u64,
    pub timestamp: Instant,
    pub torrents: Arc<[TorrentUiRecord]>,
    pub stats: SessionStats,
}

impl Default for EngineSnapshot {
    fn default() -> Self {
        Self {
            sequence: 0,
            timestamp: Instant::now(),
            torrents: Arc::new([]),
            stats: SessionStats::default(),
        }
    }
}
