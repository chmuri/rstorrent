use crate::display::{format_download_time_left, format_xb, Canvas, Window, EXTENT_FULL};
use ratatui::style::{Color, Modifier, Style};
use rt_core::Download;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailTab {
    Info,
    Peers,
    Files,
    Trackers,
    Chunks,
    Transfers,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CachedPeerDetailRow {
    pub addr: String,
    pub client: String,
    pub flags: String,
    pub rate_down_kb: f64,
    pub rate_up_kb: f64,
    pub downloaded: u64,
    pub uploaded: u64,
    pub done_pct: u64,
}

#[derive(Debug, Clone, Default)]
struct CachedPeersState {
    rows: Vec<CachedPeerDetailRow>,
    tracker_status: String,
    tracker_count: usize,
}

#[derive(Debug, Clone, Default)]
struct CachedInfoState {
    name: String,
    info_hash: String,
    downloaded: u64,
    uploaded: u64,
    total_size: u64,
    percent: u64,
    ratio: f64,
    rate_down: f64,
    rate_up: f64,
    time_left: String,
    pieces_count: usize,
    piece_length_kb: f64,
    completed_chunks: u32,
    peers_count: usize,
    trackers_count: usize,
    directory: String,
    state_str: String,
    priority: u32,
    message: String,
}

#[derive(Debug, Clone)]
pub struct CachedFileRow {
    pub idx: usize,
    pub prio_str: &'static str,
    pub size_mb: f64,
    pub comp_mb: f64,
    pub percent: u32,
    pub path: String,
}

#[derive(Debug, Clone, Default)]
struct CachedFilesState {
    single_file_fallback: Option<(f64, f64, u64, String)>,
    rows: Vec<CachedFileRow>,
}

#[derive(Debug, Clone)]
pub struct CachedTrackerRow {
    pub group: u32,
    pub tracker_type: &'static str,
    pub status: &'static str,
    pub seeds_str: String,
    pub leech_str: String,
    pub next_announce: String,
    pub url: String,
}

#[derive(Debug, Clone, Default)]
struct CachedTrackersState {
    announce_fallback: Option<String>,
    rows: Vec<CachedTrackerRow>,
}

#[derive(Debug, Clone, Default)]
struct CachedChunksState {
    total_pieces: usize,
    piece_length_kb: f64,
    completed_pieces: usize,
    active_transfers: usize,
    percent: usize,
}

#[derive(Debug, Clone)]
pub struct CachedTransferDetailRow {
    pub piece_idx: u32,
    pub finished: usize,
    pub total: usize,
    pub percent: usize,
}

#[derive(Debug, Clone, Default)]
struct CachedTransfersState {
    rows: Vec<CachedTransferDetailRow>,
}

pub struct WindowDownloadDetails {
    pub flags: u32,
    pub download: Option<Arc<RwLock<Download>>>,
    pub current_tab: DetailTab,
    pub scroll_offset: usize,
    pub page_size: usize,
    dirty: bool,
    /// Refresh interval for all tabs (permanently throttled to 1.0s by default).
    pub refresh_interval: std::time::Duration,
    /// Compatibility alias for peer refresh interval.
    pub peer_refresh_interval: std::time::Duration,
    last_refresh: Option<std::time::Instant>,
    cached_info: CachedInfoState,
    cached_peers: CachedPeersState,
    cached_files: CachedFilesState,
    cached_trackers: CachedTrackersState,
    cached_chunks: CachedChunksState,
    cached_transfers: CachedTransfersState,
}

impl Default for WindowDownloadDetails {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowDownloadDetails {
    pub fn new() -> Self {
        Self {
            flags: 0,
            download: None,
            current_tab: DetailTab::Info,
            scroll_offset: 0,
            page_size: 15,
            dirty: true,
            refresh_interval: std::time::Duration::from_secs(1),
            peer_refresh_interval: std::time::Duration::from_secs(1),
            last_refresh: None,
            cached_info: CachedInfoState::default(),
            cached_peers: CachedPeersState::default(),
            cached_files: CachedFilesState::default(),
            cached_trackers: CachedTrackersState::default(),
            cached_chunks: CachedChunksState::default(),
            cached_transfers: CachedTransfersState::default(),
        }
    }

    pub fn set_download(&mut self, download: Option<Arc<RwLock<Download>>>) {
        self.download = download;
        self.scroll_offset = 0;
        self.last_refresh = None;
        self.cached_info = CachedInfoState::default();
        self.cached_peers = CachedPeersState::default();
        self.cached_files = CachedFilesState::default();
        self.cached_trackers = CachedTrackersState::default();
        self.cached_chunks = CachedChunksState::default();
        self.cached_transfers = CachedTransfersState::default();
        self.dirty = true;
    }

    pub fn set_tab(&mut self, tab: DetailTab) {
        self.current_tab = tab;
        self.scroll_offset = 0;
        self.last_refresh = None;
        self.dirty = true;
    }

    pub fn refresh(&mut self) {
        self.last_refresh = None;
        self.dirty = true;
    }

    pub fn refresh_peers(&mut self) {
        self.refresh();
    }

    pub fn set_refresh_interval(&mut self, interval: std::time::Duration) {
        self.refresh_interval = interval;
        self.peer_refresh_interval = interval;
    }

    pub fn set_peer_refresh_interval(&mut self, interval: std::time::Duration) {
        self.set_refresh_interval(interval);
    }

    pub fn cached_peer_rows(&self) -> &[CachedPeerDetailRow] {
        &self.cached_peers.rows
    }

    pub fn scroll_up(&mut self) {
        if self.scroll_offset > 0 {
            self.scroll_offset -= 1;
            self.dirty = true;
        }
    }

    pub fn scroll_down(&mut self) {
        self.scroll_offset += 1;
        self.dirty = true;
    }

    pub fn page_up(&mut self) {
        let step = self.page_size.max(1);
        self.scroll_offset = self.scroll_offset.saturating_sub(step);
        self.dirty = true;
    }

    pub fn page_down(&mut self) {
        let step = self.page_size.max(1);
        self.scroll_offset = self.scroll_offset.saturating_add(step);
        self.dirty = true;
    }
}

impl Window for WindowDownloadDetails {
    fn flags(&self) -> u32 {
        self.flags
    }

    fn set_flags(&mut self, flags: u32) {
        self.flags = flags;
    }

    fn min_width(&self) -> u32 {
        20
    }

    fn min_height(&self) -> u32 {
        5
    }

    fn max_width(&self) -> u32 {
        EXTENT_FULL
    }

    fn max_height(&self) -> u32 {
        EXTENT_FULL
    }

    fn resize(&mut self, _x: u32, _y: u32, _width: u32, height: u32) {
        self.page_size = height.saturating_sub(3).max(1) as usize;
        self.dirty = true;
    }

    fn redraw(&mut self, canvas: &mut Canvas) {
        if canvas.is_daemon() {
            return;
        }
        canvas.erase();

        let dl_arc = match &self.download {
            Some(d) => d.clone(),
            None => {
                canvas.print_at(0, 0, "[No download selected]");
                return;
            }
        };

        let should_refresh = match self.last_refresh {
            Some(last) => last.elapsed() >= self.refresh_interval,
            None => true,
        };

        if should_refresh {
            self.last_refresh = Some(std::time::Instant::now());
            let dl = dl_arc.read().unwrap();
            let meta = dl.metadata();

            match self.current_tab {
                DetailTab::Info => {
                    let mut engine = dl.download();
                    let downloaded = engine.downloaded_bytes();
                    let uploaded = engine.uploaded_bytes();
                    let total_size = meta.total_size;
                    let percent = if total_size > 0 {
                        (downloaded * 100) / total_size
                    } else {
                        0
                    };
                    let ratio = if downloaded > 0 {
                        uploaded as f64 / downloaded as f64
                    } else {
                        0.0
                    };
                    let rate_down_bytes = engine.rate_down();
                    let rate_up_bytes = engine.rate_up();
                    let rate_down = (rate_down_bytes as f64) / 1024.0;
                    let rate_up = (rate_up_bytes as f64) / 1024.0;
                    let peers_count = engine.connection_list().size();
                    let trackers_count = engine.tracker_controller().tracker_list.size();
                    let is_done = engine.is_done();
                    let completed_chunks = engine.completed_chunks() as u32;
                    drop(engine);

                    let time_left = if rate_down_bytes > 0 && downloaded < total_size {
                        format_download_time_left(
                            total_size.saturating_sub(downloaded),
                            rate_down_bytes,
                        )
                    } else if is_done {
                        "completed".to_string()
                    } else {
                        "--d --:--".to_string()
                    };

                    self.cached_info = CachedInfoState {
                        name: meta.name.clone(),
                        info_hash: dl.info_hash_hex(),
                        downloaded,
                        uploaded,
                        total_size,
                        percent,
                        ratio,
                        rate_down,
                        rate_up,
                        time_left,
                        pieces_count: meta.pieces.len(),
                        piece_length_kb: meta.piece_length as f64 / 1024.0,
                        completed_chunks,
                        peers_count,
                        trackers_count,
                        directory: dl.directory().to_string(),
                        state_str: format!(
                            "{:?} (Active: {}, Done: {})",
                            dl.state(),
                            dl.is_active(),
                            is_done
                        ),
                        priority: dl.priority(),
                        message: dl.message().to_string(),
                    };
                }
                DetailTab::Peers => {
                    let engine = dl.download();
                    let connections = engine.connection_list();

                    let mut peer_rows: Vec<CachedPeerDetailRow> = connections
                        .iter()
                        .map(|peer| {
                            let mut rate_up = peer.rate_up.clone();
                            let mut rate_down = peer.rate_down.clone();
                            let rate_up_kb = (rate_up.rate() as f64) / 1024.0;
                            let rate_down_kb = (rate_down.rate() as f64) / 1024.0;
                            let bits = peer.bitfield.size_bits();
                            let done_pct = if bits == 0 {
                                0
                            } else {
                                (100 * peer.bitfield.size_set() as u64) / bits as u64
                            };
                            let mut flags = String::new();
                            if peer.is_encrypted {
                                flags.push('E');
                            }
                            if peer.is_obfuscated {
                                flags.push('O');
                            }
                            if peer.is_incoming() {
                                flags.push('I');
                            }
                            if peer.is_snubbed {
                                flags.push('N');
                            }
                            if peer.is_preferred {
                                flags.push('P');
                            }
                            if peer.is_banned {
                                flags.push('B');
                            }
                            if peer.is_seeder() {
                                flags.push('S');
                            }
                            if flags.is_empty() {
                                flags.push('-');
                            }

                            CachedPeerDetailRow {
                                addr: peer.socket_address.pretty_str(),
                                client: peer.client_version(),
                                flags,
                                rate_down_kb,
                                rate_up_kb,
                                downloaded: peer.downloaded,
                                uploaded: peer.uploaded,
                                done_pct,
                            }
                        })
                        .collect();

                    // Sort: actively downloading peers first (highest rate_down), then most downloaded bytes, then rate_up
                    peer_rows.sort_by(|a, b| {
                        b.rate_down_kb
                            .partial_cmp(&a.rate_down_kb)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| b.downloaded.cmp(&a.downloaded))
                            .then_with(|| {
                                b.rate_up_kb
                                    .partial_cmp(&a.rate_up_kb)
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            })
                    });

                    let tracker_controller = engine.tracker_controller();
                    let t_count = tracker_controller.tracker_list.size();
                    let status = if tracker_controller.is_requesting() {
                        "Announcing to tracker...".to_string()
                    } else if t_count == 0 {
                        "No trackers available".to_string()
                    } else {
                        "Waiting for tracker response or incoming connections".to_string()
                    };

                    self.cached_peers = CachedPeersState {
                        rows: peer_rows,
                        tracker_status: status,
                        tracker_count: t_count,
                    };
                }
                DetailTab::Files => {
                    let engine = dl.download();
                    let file_list = engine.file_list();
                    let files = file_list.files();

                    if files.is_empty() {
                        let size_mb = meta.total_size as f64 / (1024.0 * 1024.0);
                        let comp_mb = engine.downloaded_bytes() as f64 / (1024.0 * 1024.0);
                        let percent = if meta.total_size > 0 {
                            (engine.downloaded_bytes() * 100) / meta.total_size
                        } else {
                            0
                        };
                        self.cached_files = CachedFilesState {
                            single_file_fallback: Some((
                                size_mb,
                                comp_mb,
                                percent,
                                meta.name.clone(),
                            )),
                            rows: Vec::new(),
                        };
                    } else {
                        let mut rows = Vec::with_capacity(files.len());
                        for (idx, file) in files.iter().enumerate() {
                            let size_mb = file.size_bytes() as f64 / (1024.0 * 1024.0);
                            let chunks = file.size_chunks();
                            let comp_chunks = file.completed_chunks();
                            let comp_bytes = if chunks > 0 {
                                ((file.size_bytes() as u128 * comp_chunks as u128) / chunks as u128)
                                    as u64
                            } else {
                                0
                            };
                            let comp_mb = comp_bytes as f64 / (1024.0 * 1024.0);
                            let percent = if file.size_bytes() == 0 {
                                100
                            } else if chunks > 0 {
                                ((comp_chunks as u128 * 100) / chunks as u128) as u32
                            } else {
                                0
                            };
                            let prio_str = match file.priority() as i32 {
                                0 => "off",
                                2 => "high",
                                _ => "normal",
                            };
                            rows.push(CachedFileRow {
                                idx,
                                prio_str,
                                size_mb,
                                comp_mb,
                                percent,
                                path: file.path().to_string_lossy().into_owned(),
                            });
                        }
                        self.cached_files = CachedFilesState {
                            single_file_fallback: None,
                            rows,
                        };
                    }
                }
                DetailTab::Trackers => {
                    let engine = dl.download();
                    let controller = engine.tracker_controller();
                    let list = &controller.tracker_list;
                    let now = controller.cached_time();

                    if list.size() == 0 {
                        self.cached_trackers = CachedTrackersState {
                            announce_fallback: meta.announce.clone(),
                            rows: Vec::new(),
                        };
                    } else {
                        let mut rows = Vec::with_capacity(list.size());
                        for index in 0..list.size() {
                            let tracker = match list.at(index) {
                                Some(t) => t,
                                None => continue,
                            };
                            let state = tracker.state();
                            let group = tracker.group();
                            let url = tracker.url();
                            let tracker_type = if url.starts_with("udp://") {
                                "UDP"
                            } else if url.starts_with("dht://") {
                                "DHT"
                            } else {
                                "HTTP"
                            };
                            let status = if !state.is_enabled() {
                                "Disabled"
                            } else if tracker.is_requesting() {
                                "Connect"
                            } else if state.counters.failed_counter > 0
                                && state.counters.success_counter == 0
                            {
                                "Failed"
                            } else if tracker.is_usable() {
                                "Active"
                            } else {
                                "Idle"
                            };

                            let seeds_str = if state.scrape_complete > 0 {
                                format!("{:>4}", state.scrape_complete)
                            } else {
                                "  --".to_string()
                            };
                            let leech_str = if state.scrape_incomplete > 0 {
                                format!("{:>5}", state.scrape_incomplete)
                            } else {
                                "   --".to_string()
                            };

                            let next_announce = if state.counters.success_counter == 0
                                && state.counters.failed_counter == 0
                            {
                                "     --     ".to_string()
                            } else {
                                let left = state.activity_time_next().saturating_sub(now);
                                format!("{:>9}s  ", left)
                            };

                            rows.push(CachedTrackerRow {
                                group,
                                tracker_type,
                                status,
                                seeds_str,
                                leech_str,
                                next_announce,
                                url: url.to_string(),
                            });
                        }
                        self.cached_trackers = CachedTrackersState {
                            announce_fallback: None,
                            rows,
                        };
                    }
                }
                DetailTab::Chunks => {
                    let engine = dl.download();
                    let total_pieces = meta.pieces.len();
                    let completed_pieces = engine.completed_chunks();
                    let active_transfers = engine.delegator().transfers().len();
                    let percent = if total_pieces > 0 {
                        (completed_pieces * 100) / total_pieces
                    } else {
                        0
                    };
                    self.cached_chunks = CachedChunksState {
                        total_pieces,
                        piece_length_kb: meta.piece_length as f64 / 1024.0,
                        completed_pieces,
                        active_transfers,
                        percent,
                    };
                }
                DetailTab::Transfers => {
                    let engine = dl.download();
                    let transfers = engine.delegator().transfers();
                    let mut rows = Vec::with_capacity(transfers.len());
                    for block_list in transfers.iter() {
                        let piece_idx = block_list.index();
                        let finished = block_list.finished();
                        let total = block_list.len();
                        let percent = if total > 0 {
                            (finished * 100) / total
                        } else {
                            0
                        };
                        rows.push(CachedTransferDetailRow {
                            piece_idx,
                            finished,
                            total,
                            percent,
                        });
                    }
                    self.cached_transfers = CachedTransfersState { rows };
                }
            }
        }

        // Header bar with tab navigation
        let header_style = Style::default().bg(Color::Blue).fg(Color::White);
        let tab_indicator = format!(
            " [1: Info{}] [2: Peers{}] [3: Files{}] [4: Trackers{}] [5: Chunks{}] [6: Transfers{}]  (Left / Backspace to return) ",
            if self.current_tab == DetailTab::Info { " *" } else { "" },
            if self.current_tab == DetailTab::Peers { " *" } else { "" },
            if self.current_tab == DetailTab::Files { " *" } else { "" },
            if self.current_tab == DetailTab::Trackers { " *" } else { "" },
            if self.current_tab == DetailTab::Chunks { " *" } else { "" },
            if self.current_tab == DetailTab::Transfers { " *" } else { "" },
        );
        let w = canvas.width() as usize;
        let padded_header = format!("{:<width$}", tab_indicator, width = w);
        canvas.print_at_styled(0, 0, &padded_header, header_style);

        let mut y = 2u32;
        let max_y = canvas.height().saturating_sub(1);

        match self.current_tab {
            DetailTab::Info => {
                let info = &self.cached_info;
                canvas.print_at_styled(
                    0,
                    y,
                    &format!("Torrent Name:    {}", info.name),
                    Style::default().add_modifier(Modifier::BOLD),
                );
                y += 1;
                canvas.print_at(0, y, &format!("InfoHash:        {}", info.info_hash));
                y += 1;
                canvas.print_at(
                    0,
                    y,
                    &format!(
                        "Transfer:        Downloaded: {} / {} [{:>3}%] (Uploaded: {} [Ratio: {:.2}])",
                        format_xb(info.downloaded as i64),
                        format_xb(info.total_size as i64),
                        info.percent,
                        format_xb(info.uploaded as i64),
                        info.ratio
                    ),
                );
                y += 1;
                canvas.print_at(
                    0,
                    y,
                    &format!(
                        "Transfer Rate:   Down: {:6.1} KB/s  |  Up: {:6.1} KB/s  |  ETA: {}",
                        info.rate_down, info.rate_up, info.time_left
                    ),
                );
                y += 1;
                canvas.print_at(
                    0,
                    y,
                    &format!(
                        "Pieces:          {} chunks x {:.1} KB (completed: {} chunks)",
                        info.pieces_count, info.piece_length_kb, info.completed_chunks
                    ),
                );
                y += 1;
                canvas.print_at(
                    0,
                    y,
                    &format!(
                        "Connections:     {} peers connected  |  {} trackers configured",
                        info.peers_count, info.trackers_count
                    ),
                );
                y += 1;
                canvas.print_at(0, y, &format!("Save Directory:  {}", info.directory));
                y += 1;
                canvas.print_at(0, y, &format!("State:           {}", info.state_str));
                y += 1;
                canvas.print_at(0, y, &format!("Priority:        {}", info.priority));
                y += 1;
                if !info.message.is_empty() {
                    canvas.print_at(0, y, &format!("Status Message:  {}", info.message));
                }
            }
            DetailTab::Peers => {
                let width = canvas.width();
                if width >= 105 {
                    canvas.print_at_styled(
                        0,
                        y,
                        "  IP:Port             | Client               | Down KB/s | Up KB/s  | Downloaded | Uploaded | Done % | Flags ",
                        Style::default().add_modifier(Modifier::UNDERLINED),
                    );
                } else {
                    canvas.print_at_styled(
                        0,
                        y,
                        "  IP:Port            | Client          | Down KB/s | Up KB/s  | Downloaded | Flags ",
                        Style::default().add_modifier(Modifier::UNDERLINED),
                    );
                }
                y += 1;

                if self.cached_peers.rows.is_empty() {
                    canvas.print_at(
                        0,
                        y,
                        "  [No peers currently connected — contacting trackers / DHT...]",
                    );
                    y += 1;
                    canvas.print_at(
                        0,
                        y,
                        &format!(
                            "  Status: {} (Trackers configured: {})",
                            self.cached_peers.tracker_status, self.cached_peers.tracker_count
                        ),
                    );
                } else {
                    for row in self.cached_peers.rows.iter().skip(self.scroll_offset) {
                        if y >= max_y {
                            break;
                        }
                        let down_str = format_xb(row.downloaded as i64);
                        let up_str = format_xb(row.uploaded as i64);

                        let line = if width >= 105 {
                            format!(
                                "  {:<19} | {:<20} | {:>9.1} | {:>8.1} | {:>10} | {:>8} | {:>5}% | {:<5} ",
                                row.addr,
                                row.client,
                                row.rate_down_kb,
                                row.rate_up_kb,
                                down_str,
                                up_str,
                                row.done_pct,
                                row.flags
                            )
                        } else {
                            format!(
                                "  {:<18} | {:<15} | {:>9.1} | {:>8.1} | {:>10} | {:<5} ",
                                row.addr,
                                row.client,
                                row.rate_down_kb,
                                row.rate_up_kb,
                                down_str,
                                row.flags
                            )
                        };
                        canvas.print_at(0, y, &line);
                        y += 1;
                    }
                }
            }
            DetailTab::Files => {
                canvas.print_at_styled(
                    0,
                    y,
                    "  #   | Priority | Size (MB)  | Completed  | Done % | Path ",
                    Style::default().add_modifier(Modifier::UNDERLINED),
                );
                y += 1;

                if let Some((size_mb, comp_mb, percent, name)) =
                    &self.cached_files.single_file_fallback
                {
                    canvas.print_at(
                        0,
                        y,
                        &format!(
                            "   0  | normal   | {:>10.2} | {:>10.2} | {:>5}% | {}",
                            size_mb, comp_mb, percent, name
                        ),
                    );
                } else {
                    for file in self.cached_files.rows.iter().skip(self.scroll_offset) {
                        if y >= max_y {
                            break;
                        }
                        canvas.print_at(
                            0,
                            y,
                            &format!(
                                " {:<4} | {:<8} | {:>10.2} | {:>10.2} | {:>5}% | {}",
                                file.idx,
                                file.prio_str,
                                file.size_mb,
                                file.comp_mb,
                                file.percent,
                                file.path
                            ),
                        );
                        y += 1;
                    }
                }
            }
            DetailTab::Trackers => {
                canvas.print_at_styled(
                    0,
                    y,
                    "  Group | Type | Status     | Seed | Leech | Next Announce | URL ",
                    Style::default().add_modifier(Modifier::UNDERLINED),
                );
                y += 1;

                if self.cached_trackers.rows.is_empty() {
                    if let Some(announce) = &self.cached_trackers.announce_fallback {
                        canvas.print_at(
                            0,
                            y,
                            &format!(
                                "    0   | HTTP | Idle       |   -- |    -- |      --       | {}",
                                announce
                            ),
                        );
                    } else {
                        canvas.print_at(0, y, "  [No trackers configured]");
                    }
                } else {
                    for tracker in self.cached_trackers.rows.iter().skip(self.scroll_offset) {
                        if y >= max_y {
                            break;
                        }
                        canvas.print_at(
                            0,
                            y,
                            &format!(
                                "   {:>2}   | {:<4} | {:<10} | {} | {} | {}| {}",
                                tracker.group,
                                tracker.tracker_type,
                                tracker.status,
                                tracker.seeds_str,
                                tracker.leech_str,
                                tracker.next_announce,
                                tracker.url
                            ),
                        );
                        y += 1;
                    }
                }
            }
            DetailTab::Chunks => {
                canvas.print_at_styled(
                    0,
                    y,
                    "  Chunk Distribution & Integrity Overview ",
                    Style::default().add_modifier(Modifier::UNDERLINED),
                );
                y += 1;

                let chunks = &self.cached_chunks;
                canvas.print_at(
                    0,
                    y,
                    &format!(
                        "  Total Pieces:      {} (Chunk size: {:.1} KB)",
                        chunks.total_pieces, chunks.piece_length_kb
                    ),
                );
                y += 1;
                canvas.print_at(
                    0,
                    y,
                    &format!(
                        "  Completed Pieces:  {} / {} [{:>3}%]",
                        chunks.completed_pieces, chunks.total_pieces, chunks.percent
                    ),
                );
                y += 1;
                canvas.print_at(
                    0,
                    y,
                    &format!("  Active Piece Downloads: {}", chunks.active_transfers),
                );
                y += 1;

                // Visual progress bar of chunks
                let bar_width = canvas.width().saturating_sub(10).min(60) as usize;
                if bar_width > 0 && chunks.total_pieces > 0 {
                    let filled = (chunks.completed_pieces * bar_width) / chunks.total_pieces;
                    let bar = format!(
                        "  Progress: [{}{}] {:>3}%",
                        "#".repeat(filled),
                        "-".repeat(bar_width.saturating_sub(filled)),
                        chunks.percent
                    );
                    y += 1;
                    canvas.print_at_styled(0, y, &bar, Style::default().fg(Color::Green));
                }
            }
            DetailTab::Transfers => {
                canvas.print_at_styled(
                    0,
                    y,
                    "  Piece # | Finished Blocks | Total Blocks | Progress ",
                    Style::default().add_modifier(Modifier::UNDERLINED),
                );
                y += 1;

                if self.cached_transfers.rows.is_empty() {
                    canvas.print_at(0, y, "  [No active block transfers]");
                } else {
                    for block in self.cached_transfers.rows.iter().skip(self.scroll_offset) {
                        if y >= max_y {
                            break;
                        }
                        canvas.print_at(
                            0,
                            y,
                            &format!(
                                "  {:>7} | {:>15} | {:>12} | {:>7}% ",
                                block.piece_idx, block.finished, block.total, block.percent
                            ),
                        );
                        y += 1;
                    }
                }
            }
        }

        self.dirty = false;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}
