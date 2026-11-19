use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

pub type TierIndex = u8;

/// Definition of a storage tier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierDefinition {
    /// Human-readable tier name (e.g. "nvme", "ssd", "hdd").
    pub name: String,
    /// Destination base path on the filesystem.
    pub path: PathBuf,
    /// Maximum capacity limit in bytes (0 = unlimited).
    #[serde(default)]
    pub capacity_bytes: u64,
}

impl TierDefinition {
    pub fn new(name: impl Into<String>, path: impl Into<PathBuf>, capacity_bytes: u64) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            capacity_bytes,
        }
    }
}

/// Transition condition to advance a torrent to another tier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TierCondition {
    /// Torrent has resided on the current tier for at least `secs` seconds.
    TimeOnTier(u64),
    /// Share ratio has reached or exceeded `ratio` (e.g. 1.0 = 100%, 2.0 = 200%).
    MinRatio(f64),
    /// Torrent download has fully completed (100% verified pieces).
    IsComplete,
    /// Total uploaded bytes across all time has reached or exceeded `bytes`.
    MinUploaded(u64),
    /// Torrent has been stopped/inactive for at least `secs` seconds.
    InactiveSince(u64),
}

/// Transition rule between two tiers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TierTransitionRule {
    pub from_tier: TierIndex,
    pub to_tier: TierIndex,
    pub conditions: Vec<TierCondition>,
}

impl TierTransitionRule {
    pub fn new(from_tier: TierIndex, to_tier: TierIndex, conditions: Vec<TierCondition>) -> Self {
        Self {
            from_tier,
            to_tier,
            conditions,
        }
    }
}

/// Overall tiering configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TieringConfig {
    pub enabled: bool,
    pub check_interval_secs: u64,
    pub tiers: Vec<TierDefinition>,
    pub rules: Vec<TierTransitionRule>,
}

impl Default for TieringConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            check_interval_secs: 300,
            tiers: Vec::new(),
            rules: Vec::new(),
        }
    }
}

/// Historical record of a tier transition for a torrent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierTransitionRecord {
    pub from_tier: TierIndex,
    pub to_tier: TierIndex,
    pub timestamp: u64,
    pub reason: String,
}

/// Per-torrent tier state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TorrentTierState {
    pub current_tier: Option<TierIndex>,
    pub tier_entered_at: u64,
    pub transition_history: Vec<TierTransitionRecord>,
}

impl TorrentTierState {
    pub fn new_on_tier(tier: TierIndex, now: u64) -> Self {
        Self {
            current_tier: Some(tier),
            tier_entered_at: now,
            transition_history: Vec::new(),
        }
    }

    pub fn record_transition(&mut self, to_tier: TierIndex, now: u64, reason: String) {
        if let Some(from) = self.current_tier {
            self.transition_history.push(TierTransitionRecord {
                from_tier: from,
                to_tier,
                timestamp: now,
                reason,
            });
        }
        self.current_tier = Some(to_tier);
        self.tier_entered_at = now;
    }
}

/// Live status of a move operation for a torrent.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum MoveStatus {
    #[default]
    Idle,
    InProgress {
        target_path: PathBuf,
        started_at: u64,
    },
    Completed {
        from_path: PathBuf,
        to_path: PathBuf,
        timestamp: u64,
    },
    Failed {
        target_path: PathBuf,
        error: String,
        timestamp: u64,
    },
}

#[derive(Debug, Error)]
pub enum MoveError {
    #[error("source path does not exist: {0}")]
    SourceNotFound(PathBuf),

    #[error("target path already exists: {0}")]
    TargetAlreadyExists(PathBuf),

    #[error("insufficient disk space: needed {required} bytes, available {available} bytes")]
    InsufficientDiskSpace { required: u64, available: u64 },

    #[error("I/O error during file move: {0}")]
    Io(#[from] std::io::Error),

    #[error("torrent is in an invalid state for move: {0}")]
    TorrentBusy(String),

    #[error("engine error: {0}")]
    Engine(String),
}
