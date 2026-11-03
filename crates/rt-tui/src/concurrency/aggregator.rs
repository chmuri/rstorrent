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
    let downloads = download_list.downloads();
    let mut records = Vec::with_capacity(downloads.len());

    for d_arc in downloads {
        let d = match d_arc.read() {
            Ok(guard) => guard,
            Err(_) => continue,
        };

        let hash_bytes = *d.info_hash().as_bytes();
        let hash_str = d.info_hash_hex();

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

/// Helper that samples `DownloadList`, calculates aggregate `SessionStats`, and extracts an immutable `EngineSnapshot` in a single pass.
pub fn sample_engine_snapshot_with_stats(
    download_list: &DownloadList,
    listen_port: u16,
    sequence: u64,
) -> EngineSnapshot {
    let downloads = download_list.downloads();
    let mut records = Vec::with_capacity(downloads.len());

    let mut active_count = 0u32;
    let mut global_down = 0u64;
    let mut global_up = 0u64;

    for d_arc in downloads {
        let d = match d_arc.read() {
            Ok(guard) => guard,
            Err(_) => continue,
        };

        let hash_bytes = *d.info_hash().as_bytes();
        let hash_str = d.info_hash_hex();

        let meta = d.metadata();
        let size_bytes = meta.total_size;
        let completed_bytes = d.completed_bytes();
        let down_rate_bps = d.rate_down();
        let up_rate_bps = d.rate_up();
        let total_uploaded_bytes = d.uploaded_bytes();

        global_down += down_rate_bps;
        global_up += up_rate_bps;

        let state = if d.is_error() {
            TorrentUiState::Error
        } else if d.is_paused() || d.is_stopped() {
            TorrentUiState::Paused
        } else if d.is_downloading() {
            active_count += 1;
            TorrentUiState::Downloading
        } else if d.is_seeding() {
            TorrentUiState::Seeding
        } else if d.is_done() {
            TorrentUiState::Complete
        } else {
            TorrentUiState::Active
        };

        if (down_rate_bps > 0 || up_rate_bps > 0) && state != TorrentUiState::Downloading {
            active_count += 1;
        }

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

    #[allow(clippy::unnecessary_cast)]
    let (disk_free_bytes, disk_total_bytes) = match nix::sys::statvfs::statvfs(".") {
        Ok(stat) => {
            let bsize = stat.block_size() as u64;
            let free = stat.blocks_available() as u64 * bsize;
            let total = stat.blocks() as u64 * bsize;
            (free, total)
        }
        Err(_) => (480 * 1024 * 1024 * 1024, 1000 * 1024 * 1024 * 1024),
    };

    let stats = SessionStats {
        global_down_bps: global_down,
        global_up_bps: global_up,
        global_down_total: 0,
        global_up_total: 0,
        disk_free_bytes,
        disk_total_bytes,
        dht_nodes: 0,
        listen_port,
        port_open: true,
        total_torrents: downloads.len() as u32,
        active_torrents: active_count,
    };

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

    // 4. Piece Map & In-Flight Pieces
    let completed_bf = eng_guard.chunk_selector().completed();
    let transfers = eng_guard.delegator().transfers();
    let chunk_stats = eng_guard.chunk_statistics();

    let mut in_flight_pieces = Vec::new();
    let mut downloading_set = std::collections::HashSet::new();

    for bl in transfers.iter() {
        let piece_idx = bl.index();
        let finished_blocks = bl.finished() as u16;
        let total_blocks = bl.len() as u16;
        let block_size = 16_384u64;
        let bytes_completed = (finished_blocks as u64).saturating_mul(block_size);
        let bytes_total = (total_blocks as u64).saturating_mul(block_size);

        let peer_addr = bl.blocks().iter().find_map(|b| {
            b.transfers()
                .first()
                .map(|t| format!("Peer #{}", t.peer_id()))
        });

        downloading_set.insert(piece_idx);
        in_flight_pieces.push(crate::state::InFlightPieceRecord {
            piece_index: piece_idx,
            blocks_completed: finished_blocks,
            blocks_total: total_blocks,
            bytes_completed,
            bytes_total,
            peer_addr,
        });
    }

    let mut piece_map = Vec::with_capacity(meta.num_pieces);
    for i in 0..meta.num_pieces {
        if completed_bf.get(i) {
            piece_map.push(crate::state::PieceState::Verified);
        } else if downloading_set.contains(&(i as u32)) {
            piece_map.push(crate::state::PieceState::Downloading);
        } else if chunk_stats.rarity(i) > 0 {
            piece_map.push(crate::state::PieceState::PeerAvailable);
        } else {
            piece_map.push(crate::state::PieceState::Missing);
        }
    }

    let state_name = format!("{:?}", eng_guard.state());
    drop(eng_guard);

    // 5. Logs
    let log_messages = vec![
        format!("Torrent: {}", d.name()),
        format!("Save path: {}", d.directory()),
        format!("State: {}", state_name),
        format!("Hash: {}", d.info_hash_hex()),
    ];

    Some(crate::state::TorrentDetailSnapshot {
        peers,
        trackers,
        files,
        piece_map,
        in_flight_pieces,
        log_messages,
    })
}
