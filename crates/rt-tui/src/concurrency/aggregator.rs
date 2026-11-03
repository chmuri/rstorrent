//! Engine state aggregator and snapshot broadcaster over lock-free watch channel.

use crate::state::{EngineSnapshot, SessionStats, TorrentUiRecord, TorrentUiState};
use rt_core::DownloadList;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;

/// Broadcast channel for engine snapshots
pub struct EngineSnapshotBroadcaster {
    tx: watch::Sender<Arc<EngineSnapshot>>,
    rx: watch::Receiver<Arc<EngineSnapshot>>,
}

impl EngineSnapshotBroadcaster {
    pub fn new() -> Self {
        let initial = Arc::new(EngineSnapshot::default());
        let (tx, rx) = watch::channel(initial);
        Self { tx, rx }
    }

    pub fn subscribe(&self) -> watch::Receiver<Arc<EngineSnapshot>> {
        self.rx.clone()
    }

    pub fn publish(&self, snapshot: EngineSnapshot) {
        let _ = self.tx.send(Arc::new(snapshot));
    }
}

impl Default for EngineSnapshotBroadcaster {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper that samples `DownloadList` and extracts an immutable `EngineSnapshot`.
pub fn sample_engine_snapshot(
    download_list: &DownloadList,
    stats: SessionStats,
    sequence: u64,
) -> EngineSnapshot {
    let downloads = download_list.all();
    let mut records = Vec::with_capacity(downloads.len());

    for d_arc in &downloads {
        let d = match d_arc.read() {
            Ok(guard) => guard,
            Err(_) => continue,
        };

        let hash_str = d.info_hash_hex();
        let hash_bytes = lt_common::hash_string::HashString::from_hex(&hash_str)
            .map(|h| *h.as_bytes())
            .unwrap_or([0u8; 20]);

        let meta = d.metadata();
        let size_bytes = meta.total_size;
        let completed_bytes = d.completed_bytes();
        let down_rate_bps = d.rate_down();
        let up_rate_bps = d.rate_up();
        let total_uploaded_bytes = d.uploaded_bytes();

        let state = if d.is_error() {
            TorrentUiState::Error
        } else if d.is_paused() || d.is_stopped() {
            TorrentUiState::Paused
        } else if d.is_downloading() {
            TorrentUiState::Downloading
        } else if d.is_seeding() {
            TorrentUiState::Seeding
        } else if d.is_done() {
            TorrentUiState::Complete
        } else {
            TorrentUiState::Active
        };

        let eng_guard = d.download();
        let seeds_connected = eng_guard.seeder_peer_count() as u32;
        let peers_connected = eng_guard.peer_count() as u32;
        let seeds_total = seeds_connected;
        let peers_total = peers_connected;
        let verified_pieces = eng_guard.completed_chunks() as u32;
        drop(eng_guard);

        let eta_seconds = if down_rate_bps > 0 && size_bytes > completed_bytes {
            Some((size_bytes - completed_bytes) / down_rate_bps)
        } else {
            None
        };

        let ratio_promille = if completed_bytes > 0 {
            ((total_uploaded_bytes as f64 / completed_bytes as f64) * 1000.0) as u32
        } else {
            0
        };

        let piece_size = meta.piece_length as u64;
        let total_pieces = meta.num_pieces as u32;

        records.push(TorrentUiRecord {
            hash: hash_bytes,
            hash_hex: hash_str,
            name: d.name().to_string(),
            size_bytes,
            completed_bytes,
            down_rate_bps,
            up_rate_bps,
            total_uploaded_bytes,
            state,
            seeds_connected,
            seeds_total,
            peers_connected,
            peers_total,
            priority: 1,
            eta_seconds,
            ratio_promille,
            tags: Vec::new(),
            piece_size,
            total_pieces,
            verified_pieces,
            save_path: d.directory().to_string(),
            is_private: meta.is_private,
            message: if d.message().is_empty() {
                None
            } else {
                Some(d.message().to_string())
            },
        });
    }

    EngineSnapshot {
        sequence,
        timestamp: Instant::now(),
        torrents: records.into(),
        stats,
    }
}

/// Samples detailed diagnostics for a specific focused torrent.
pub fn sample_torrent_details(
    download_list: &DownloadList,
    hash: &[u8; 20],
) -> Option<crate::state::TorrentDetailSnapshot> {
    let hs = lt_common::hash_string::HashString::new(*hash);
    let dl_arc = download_list.find(&hs)?;
    let d = dl_arc.read().ok()?;

    let meta = d.metadata();
    let eng_guard = d.download();

    // 1. Files
    let mut files = Vec::new();
    let file_list = eng_guard.file_list();
    for (idx, f) in file_list.files().iter().enumerate() {
        let completed = (f.completed_chunks() as u64)
            .saturating_mul(meta.piece_length as u64)
            .min(f.size_bytes());
        files.push(crate::state::FileUiRecord {
            index: idx,
            path: f.path().to_string_lossy().to_string(),
            size_bytes: f.size_bytes(),
            completed_bytes: completed,
            priority: f.priority() as u8,
        });
    }

    // 2. Peers
    let mut peers = Vec::new();
    let connections = eng_guard.connection_list();
    for peer in connections.iter() {
        let mut rate_up = peer.rate_up.clone();
        let mut rate_down = peer.rate_down.clone();
        let down_rate_bps = rate_down.rate();
        let up_rate_bps = rate_up.rate();
        let bits = peer.bitfield.size_bits();
        let progress_percent = if bits == 0 {
            0
        } else {
            ((100 * peer.bitfield.size_set() as u64) / bits as u64).min(100) as u8
        };
        peers.push(crate::state::PeerUiRecord {
            ip_port: peer.socket_address.pretty_str(),
            client_name: peer.client_version(),
            flags: if peer.is_seeder() {
                "S".into()
            } else {
                "u/d".into()
            },
            down_rate_bps,
            up_rate_bps,
            progress_percent,
            is_encrypted: peer.is_encrypted,
        });
    }

    // 3. Trackers
    let mut trackers = Vec::new();
    let tracker_ctrl = eng_guard.tracker_controller();
    let tr_list = &tracker_ctrl.tracker_list;
    for index in 0..tr_list.size() {
        if let Some(tr) = tr_list.at(index) {
            let state = tr.state();
            let status = if !state.is_enabled() {
                "Disabled".to_string()
            } else if tr.is_requesting() {
                "Connecting".to_string()
            } else if tr.is_usable() {
                "Active".to_string()
            } else {
                "Idle".to_string()
            };
            trackers.push(crate::state::TrackerUiRecord {
                index,
                tier: tr.group(),
                url: tr.url().to_string(),
                status,
                seeds: state.scrape_complete,
                peers: state.scrape_incomplete,
                interval_secs: state.normal_interval as u32,
                next_announce_secs: 0,
            });
        }
    }

    // 4. Piece Map
    let mut piece_map = Vec::with_capacity(meta.num_pieces);
    let completed_pieces = eng_guard.completed_chunks();
    for i in 0..meta.num_pieces {
        if i < completed_pieces {
            piece_map.push(crate::state::PieceState::Verified);
        } else {
            piece_map.push(crate::state::PieceState::Missing);
        }
    }

    // 5. Logs
    let log_messages = vec![
        format!("Torrent: {}", d.name()),
        format!("Save path: {}", d.directory()),
        format!("State: {:?}", d.state()),
        format!("Hash: {}", d.info_hash_hex()),
    ];

    drop(eng_guard);

    Some(crate::state::TorrentDetailSnapshot {
        peers,
        trackers,
        files,
        piece_map,
        log_messages,
    })
}
