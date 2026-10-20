//! Chunks-seen window.
//!
//! Displays a piece-by-peer availability matrix for the focused download. Each
//! logical band covers `columns` pieces; every band contains one row per peer.
//! Only the rows and cells that are currently visible are visited, so rendering
//! is O(width * height) even when a torrent has hundreds of thousands of
//! pieces.
//!
//! The window never keeps engine locks while drawing. [`ChunksSeenWindow::refresh`]
//! takes a small value-based snapshot using only public `lt-engine` accessors.
//! `lt-protocol`/`lt-storage` types are reached through those accessors without
//! naming them, which keeps `rt-tui`'s direct dependency list unchanged.

use crate::display::{Canvas, Window, EXTENT_FULL};
use lt_common::{Bitfield, HashString};
use ratatui::style::{Color, Modifier, Style};
use rt_core::Download;
use std::sync::{Arc, RwLock};

/// Number of terminal rows reserved for the title (grid starts on row 1).
const HEADER_ROWS: u32 = 1;

/// Colour-independent cell state. The symbols are deliberately distinct so the
/// view remains readable on monochrome terminals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkPresence {
    /// Neither we nor the peer have the piece.
    Missing,
    /// The peer advertises the piece but we do not have it yet.
    PeerHas,
    /// We already completed and verified the piece.
    LocalComplete,
    /// The piece has at least one outstanding block transfer.
    Downloading,
}

impl ChunkPresence {
    pub fn symbol(self) -> char {
        match self {
            ChunkPresence::Missing => '.',
            ChunkPresence::PeerHas => '+',
            ChunkPresence::LocalComplete => '#',
            ChunkPresence::Downloading => '>',
        }
    }

    pub fn style(self) -> Style {
        match self {
            ChunkPresence::Missing => Style::default().fg(Color::DarkGray),
            ChunkPresence::PeerHas => Style::default().fg(Color::Cyan),
            ChunkPresence::LocalComplete => Style::default().fg(Color::Green),
            ChunkPresence::Downloading => Style::default().fg(Color::Yellow),
        }
    }
}

/// A detached snapshot of one peer's piece availability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkPeerRow {
    pub peer_id: HashString,
    pub address: String,
    /// Bits above the window's `piece_count` are ignored.
    pub pieces: Bitfield,
    pub is_seed: bool,
    pub is_banned: bool,
}

impl ChunkPeerRow {
    pub fn new(peer_id: HashString, address: impl Into<String>, pieces: Bitfield) -> Self {
        let is_seed = pieces.is_all_set() && !pieces.is_empty();
        Self {
            peer_id,
            address: address.into(),
            pieces,
            is_seed,
            is_banned: false,
        }
    }

    pub fn with_flags(mut self, is_seed: bool, is_banned: bool) -> Self {
        self.is_seed = is_seed;
        self.is_banned = is_banned;
        self
    }

    pub fn has_piece(&self, index: usize) -> bool {
        self.pieces.get(index)
    }

    /// Short label used when the terminal is too narrow for an address.
    pub fn short_id(&self) -> String {
        let hex = self.peer_id.to_hex();
        hex[..8.min(hex.len())].to_string()
    }
}

/// Snapshot of the live download used to refresh the window.
struct ChunksSeenSnapshot {
    name: String,
    piece_count: usize,
    complete: u32,
    accounted: u32,
    peers: Vec<ChunkPeerRow>,
    completed: Bitfield,
    active_pieces: Vec<bool>,
}

/// Standalone chunks-seen grid window.
pub struct ChunksSeenWindow {
    pub flags: u32,
    pub download: Option<Arc<RwLock<Download>>>,
    pub download_name: String,
    pub peers: Vec<ChunkPeerRow>,
    /// Pieces we have verified locally.
    pub completed: Bitfield,
    /// Pieces with at least one outstanding block transfer.
    pub active_pieces: Vec<bool>,
    pub piece_count: usize,
    /// Pieces per band; recomputed from the canvas width on every draw/resize.
    pub columns: usize,
    /// Band containing the focused row.
    pub focused_band: usize,
    /// Peer row inside `focused_band`.
    pub focused_peer: usize,
    /// First visible virtual row (band * peer_count + peer).
    pub scroll_row: usize,
    pub page_size: usize,
    /// When true, `redraw` re-snapshots the live download at the configured interval.
    pub auto_refresh: bool,
    /// Throttling interval for auto-refreshing the chunks seen grid (default 1.0s).
    pub refresh_interval: std::time::Duration,
    last_refresh: Option<std::time::Instant>,
    /// Total peers that reported a complete bitfield (seeders).
    pub complete_peers: u32,
    /// Peers included in the rarity accounting (non-seeders).
    pub accounted_peers: u32,
    last_width: u32,
    dirty: bool,
}

impl Default for ChunksSeenWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl ChunksSeenWindow {
    pub fn new() -> Self {
        Self {
            flags: 0,
            download: None,
            download_name: String::new(),
            peers: Vec::new(),
            completed: Bitfield::new(0),
            active_pieces: Vec::new(),
            piece_count: 0,
            columns: 1,
            focused_band: 0,
            focused_peer: 0,
            scroll_row: 0,
            page_size: 10,
            auto_refresh: true,
            refresh_interval: std::time::Duration::from_secs(1),
            last_refresh: None,
            complete_peers: 0,
            accounted_peers: 0,
            last_width: 80,
            dirty: true,
        }
    }

    /// Bind to a live download. A different pointer resets navigation; binding
    /// the same download again simply refreshes the snapshot.
    pub fn set_download(&mut self, download: Option<Arc<RwLock<Download>>>) {
        let changed = match (&self.download, &download) {
            (Some(old), Some(new)) => !Arc::ptr_eq(old, new),
            (None, None) => false,
            _ => true,
        };

        self.download = download;
        if changed {
            self.download_name.clear();
            self.peers.clear();
            self.completed = Bitfield::new(0);
            self.active_pieces.clear();
            self.piece_count = 0;
            self.complete_peers = 0;
            self.accounted_peers = 0;
            self.focused_band = 0;
            self.focused_peer = 0;
            self.scroll_row = 0;
        }
        self.last_refresh = None;
        self.refresh();
    }

    pub fn set_refresh_interval(&mut self, interval: std::time::Duration) {
        self.refresh_interval = interval;
    }

    pub fn invalidate_refresh(&mut self) {
        self.last_refresh = None;
        self.dirty = true;
    }

    /// Install a detached peer snapshot. Primarily useful for tests and for
    /// callers that already have a view model.
    pub fn set_peers(&mut self, peers: Vec<ChunkPeerRow>) {
        let peer_piece_count = peers
            .iter()
            .map(|peer| peer.pieces.size_bits())
            .max()
            .unwrap_or(0);
        self.piece_count = self.piece_count.max(peer_piece_count);
        self.resize_auxiliary_bits();
        self.peers = peers;
        self.clamp_focus();
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    pub fn set_piece_count(&mut self, piece_count: usize) {
        if piece_count == self.piece_count {
            return;
        }
        self.piece_count = piece_count;
        self.resize_auxiliary_bits();
        self.clamp_focus();
        self.dirty = true;
    }

    /// Replace the locally completed bitfield. Also grows `piece_count` when
    /// the supplied bitfield is larger than the current grid.
    pub fn set_completed(&mut self, completed: Bitfield) {
        self.piece_count = self.piece_count.max(completed.size_bits());
        self.completed = completed;
        self.resize_auxiliary_bits();
        self.clamp_focus();
        self.dirty = true;
    }

    /// Mark pieces that currently have outstanding transfers.
    pub fn set_active_pieces(&mut self, pieces: impl IntoIterator<Item = u32>) {
        let mut max_index = 0usize;
        let mut indices = Vec::new();
        for piece in pieces {
            let index = piece as usize;
            max_index = max_index.max(index.saturating_add(1));
            indices.push(index);
        }
        self.piece_count = self.piece_count.max(max_index);
        self.resize_auxiliary_bits();
        for index in indices {
            if let Some(active) = self.active_pieces.get_mut(index) {
                *active = true;
            }
        }
        self.dirty = true;
    }

    pub fn set_auto_refresh(&mut self, auto_refresh: bool) {
        self.auto_refresh = auto_refresh;
        self.dirty = true;
    }

    pub fn set_page_size(&mut self, page_size: usize) {
        self.page_size = page_size.max(1);
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    /// Recompute the grid layout for `width` terminal columns without drawing.
    pub fn set_layout_width(&mut self, width: u32) {
        self.columns = Self::columns_for_width(width);
        self.last_width = width;
        self.clamp_focus();
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    pub fn len(&self) -> usize {
        self.peers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.peers.is_empty() || self.piece_count == 0
    }

    pub fn piece_columns(&self) -> usize {
        self.columns.max(1)
    }

    /// Number of pieces that fit in one band at `width` columns.
    pub fn columns_for_width(width: u32) -> usize {
        (width as usize)
            .saturating_sub(Self::label_width_for(width))
            .max(1)
    }

    /// Width of the peer label column, including its trailing separator.
    pub fn label_width_for(width: u32) -> usize {
        if width <= 8 {
            0
        } else {
            ((width as usize) / 4).clamp(8, 20).min(width as usize - 1)
        }
    }

    pub fn label_width(&self) -> usize {
        Self::label_width_for(self.last_width)
    }

    pub fn band_count(&self) -> usize {
        if self.piece_count == 0 || self.peers.is_empty() {
            0
        } else {
            (self.piece_count + self.piece_columns() - 1) / self.piece_columns()
        }
    }

    pub fn row_count(&self) -> usize {
        self.band_count().saturating_mul(self.peers.len())
    }

    pub fn focus_row(&self) -> usize {
        self.focused_band
            .saturating_mul(self.peers.len())
            .saturating_add(self.focused_peer)
    }

    pub fn selected_peer(&self) -> Option<&ChunkPeerRow> {
        self.peers.get(self.focused_peer)
    }

    pub fn select_index(&mut self, band: usize, peer: usize) -> bool {
        if self.peers.is_empty() || self.band_count() == 0 {
            return false;
        }
        if band >= self.band_count() || peer >= self.peers.len() {
            return false;
        }
        self.focused_band = band;
        self.focused_peer = peer;
        self.ensure_visible(self.page_size);
        self.dirty = true;
        true
    }

    pub fn move_up(&mut self) -> bool {
        let current = self.focus_row();
        if current == 0 || self.row_count() == 0 {
            return false;
        }
        self.set_focus_row(current - 1);
        true
    }

    pub fn move_down(&mut self) -> bool {
        if self.row_count() == 0 {
            return false;
        }
        let current = self.focus_row();
        if current + 1 >= self.row_count() {
            return false;
        }
        self.set_focus_row(current + 1);
        true
    }

    pub fn page_up(&mut self) -> bool {
        let current = self.focus_row();
        if current == 0 {
            return false;
        }
        self.set_focus_row(current.saturating_sub(self.page_size));
        true
    }

    pub fn page_down(&mut self) -> bool {
        if self.row_count() == 0 {
            return false;
        }
        let current = self.focus_row();
        let target = (current + self.page_size).min(self.row_count() - 1);
        if target == current {
            return false;
        }
        self.set_focus_row(target);
        true
    }

    pub fn focus_first(&mut self) {
        if self.row_count() > 0 {
            self.set_focus_row(0);
        }
    }

    pub fn focus_last(&mut self) {
        let count = self.row_count();
        if count > 0 {
            self.set_focus_row(count - 1);
        }
    }

    /// Keep `focus_row` inside the visible window of `visible_rows` rows.
    pub fn ensure_visible(&mut self, visible_rows: usize) {
        let total = self.row_count();
        if visible_rows == 0 || total == 0 {
            self.scroll_row = 0;
            return;
        }
        let row = self.focus_row().min(total - 1);
        if row < self.scroll_row {
            self.scroll_row = row;
        } else if row >= self.scroll_row + visible_rows {
            self.scroll_row = row + 1 - visible_rows;
        }
        let max_scroll = total.saturating_sub(visible_rows);
        self.scroll_row = self.scroll_row.min(max_scroll);
    }

    /// Re-snapshot the live download's availability matrix.
    pub fn refresh(&mut self) {
        self.last_refresh = Some(std::time::Instant::now());
        let focus = (self.focused_band, self.focused_peer);
        match self.snapshot_live() {
            Some(snapshot) => {
                self.download_name = snapshot.name;
                self.peers = snapshot.peers;
                self.completed = snapshot.completed;
                self.active_pieces = snapshot.active_pieces;
                self.piece_count = snapshot.piece_count;
                self.complete_peers = snapshot.complete;
                self.accounted_peers = snapshot.accounted;
                self.focused_band = focus.0;
                self.focused_peer = focus.1;
            }
            None => {
                self.download_name.clear();
                self.peers.clear();
                self.completed = Bitfield::new(0);
                self.active_pieces.clear();
                self.piece_count = 0;
                self.complete_peers = 0;
                self.accounted_peers = 0;
                self.focused_band = 0;
                self.focused_peer = 0;
                self.scroll_row = 0;
            }
        }
        self.clamp_focus();
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    /// Classify one grid cell. `None` when either the peer or the piece is out
    /// of range.
    pub fn presence_at(&self, peer_index: usize, piece_index: usize) -> Option<ChunkPresence> {
        let peer = self.peers.get(peer_index)?;
        if piece_index >= self.piece_count {
            return None;
        }
        if self.completed.get(piece_index) {
            Some(ChunkPresence::LocalComplete)
        } else if self
            .active_pieces
            .get(piece_index)
            .copied()
            .unwrap_or(false)
        {
            Some(ChunkPresence::Downloading)
        } else if peer.pieces.get(piece_index) {
            Some(ChunkPresence::PeerHas)
        } else {
            Some(ChunkPresence::Missing)
        }
    }

    /// Convenience for tests: the exact glyph and style used for a cell.
    pub fn cell_at(&self, peer_index: usize, piece_index: usize) -> Option<(char, Style)> {
        let presence = self.presence_at(peer_index, piece_index)?;
        Some((presence.symbol(), presence.style()))
    }

    fn set_focus_row(&mut self, row: usize) {
        let peers = self.peers.len();
        let total = self.row_count();
        if peers == 0 || total == 0 {
            self.focused_band = 0;
            self.focused_peer = 0;
            self.scroll_row = 0;
            return;
        }
        let row = row.min(total - 1);
        self.focused_band = row / peers;
        self.focused_peer = row % peers;
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    fn resize_auxiliary_bits(&mut self) {
        if self.completed.size_bits() != self.piece_count {
            let completed = Bitfield::from_bytes(self.completed.as_bytes(), self.piece_count);
            self.completed = completed;
        }
        if self.active_pieces.len() != self.piece_count {
            self.active_pieces.resize(self.piece_count, false);
        }
    }

    fn clamp_focus(&mut self) {
        let bands = self.band_count();
        if self.peers.is_empty() || bands == 0 {
            self.focused_band = 0;
            self.focused_peer = 0;
            self.scroll_row = 0;
            return;
        }
        self.focused_band = self.focused_band.min(bands - 1);
        self.focused_peer = self.focused_peer.min(self.peers.len() - 1);
        let max_row = self.row_count().saturating_sub(1);
        self.scroll_row = self.scroll_row.min(max_row);
    }

    /// Build a value snapshot while holding the engine lock only for the copy.
    fn snapshot_live(&self) -> Option<ChunksSeenSnapshot> {
        let download = self.download.as_ref()?;
        let guard = download.read().ok()?;
        let engine = guard.download();
        let statistics = engine.chunk_statistics();
        let connections = engine.connection_list();

        let piece_count = statistics
            .len()
            .max(engine.chunk_selector().size())
            .max(engine.total_chunks());

        let completed = engine.chunk_selector().completed().clone();

        let mut peers: Vec<ChunkPeerRow> = statistics
            .available()
            .peers()
            .map(|(peer_id, availability)| {
                let address = connections
                    .find_by_id(peer_id)
                    .map(|connection| connection.socket_address.pretty_str())
                    .unwrap_or_else(|| format!("peer {}", &peer_id.to_hex()[..8]));
                let is_banned = connections
                    .find_by_id(peer_id)
                    .map(|connection| connection.is_banned)
                    .unwrap_or(false);
                ChunkPeerRow {
                    peer_id: *peer_id,
                    address,
                    pieces: availability.bitfield.clone(),
                    is_seed: availability.bitfield.is_all_set()
                        && !availability.bitfield.is_empty(),
                    is_banned,
                }
            })
            .collect();
        peers.sort_by(|a, b| {
            a.address
                .cmp(&b.address)
                .then_with(|| a.peer_id.cmp(&b.peer_id))
        });

        let mut active_pieces = vec![false; piece_count];
        for block_list in engine.delegator().transfers().iter() {
            let index = block_list.index() as usize;
            if index >= piece_count {
                continue;
            }
            let active = block_list.blocks().iter().any(|block| {
                !block.is_finished()
                    && block
                        .transfers()
                        .iter()
                        .any(|transfer| !transfer.is_erased())
            });
            if active {
                active_pieces[index] = true;
            }
        }

        Some(ChunksSeenSnapshot {
            name: guard.name().to_string(),
            piece_count,
            complete: statistics.complete(),
            accounted: statistics.accounted(),
            peers,
            completed,
            active_pieces,
        })
    }

    fn render_rows(&mut self, canvas: &mut Canvas) {
        let visible_rows = canvas.height().saturating_sub(HEADER_ROWS) as usize;
        if visible_rows == 0 {
            return;
        }

        self.ensure_visible(visible_rows);

        let total = self.row_count();
        let peers = self.peers.len();
        if total == 0 || peers == 0 {
            return;
        }

        let end = (self.scroll_row + visible_rows).min(total);
        let label_width = self.label_width();
        let columns = self.piece_columns();

        for (offset, row) in (self.scroll_row..end).enumerate() {
            let band = row / peers;
            let peer_index = row % peers;
            let peer = &self.peers[peer_index];
            let y = HEADER_ROWS + offset as u32;
            if y >= canvas.height() {
                break;
            }
            let selected = band == self.focused_band && peer_index == self.focused_peer;

            if label_width > 0 {
                let marker = if selected { '>' } else { ' ' };
                let label = fit_label(&format!("{}{}", marker, peer.address), label_width);
                let label_style = if selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else if peer.is_banned {
                    Style::default().fg(Color::Red)
                } else if peer.is_seed {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                };
                canvas.print_at_styled(0, y, &label, label_style);
            }

            let start_piece = band.saturating_mul(columns);
            for column in 0..columns {
                let piece = start_piece + column;
                if piece >= self.piece_count {
                    break;
                }
                let (symbol, style) = match self.presence_at(peer_index, piece) {
                    Some(presence) => (presence.symbol(), presence.style()),
                    None => (' ', Style::default()),
                };
                canvas.print_char(label_width as u32 + column as u32, y, symbol, style);
            }
        }
    }

    fn render_title(&self, canvas: &mut Canvas) {
        let name = if self.download_name.is_empty() {
            "?"
        } else {
            self.download_name.as_str()
        };
        let title = format!(
            "[Chunks seen: {}] {} pieces, {} peers",
            name,
            self.piece_count,
            self.peers.len()
        );
        canvas.print_at(0, 0, &title);

        if self.piece_count == 0 || self.peers.is_empty() {
            return;
        }

        let bands = self.band_count();
        let band = self.focused_band.min(bands.saturating_sub(1));
        let columns = self.piece_columns();
        let first = band.saturating_mul(columns);
        let last = (first + columns).min(self.piece_count).saturating_sub(1);
        let counter = format!("[{}/{} pieces {}-{}]", band + 1, bands, first, last);

        let used = title.len() + counter.len() + 2;
        if (canvas.width() as usize) > used {
            let x = canvas.width().saturating_sub(counter.len() as u32);
            canvas.print_at(x, 0, &counter);
        }
    }
}

impl Window for ChunksSeenWindow {
    fn flags(&self) -> u32 {
        self.flags
    }

    fn set_flags(&mut self, flags: u32) {
        self.flags = flags;
    }

    fn min_width(&self) -> u32 {
        8
    }

    fn min_height(&self) -> u32 {
        2
    }

    fn max_width(&self) -> u32 {
        EXTENT_FULL
    }

    fn max_height(&self) -> u32 {
        EXTENT_FULL
    }

    fn resize(&mut self, _x: u32, _y: u32, width: u32, height: u32) {
        self.page_size = height.saturating_sub(HEADER_ROWS).max(1) as usize;
        self.columns = Self::columns_for_width(width);
        self.last_width = width;
        self.clamp_focus();
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    fn redraw(&mut self, canvas: &mut Canvas) {
        if canvas.is_daemon() {
            return;
        }
        canvas.erase();

        self.columns = Self::columns_for_width(canvas.width());
        self.last_width = canvas.width();
        self.page_size = canvas.height().saturating_sub(HEADER_ROWS).max(1) as usize;
        if self.auto_refresh && self.download.is_some() {
            let should_refresh = match self.last_refresh {
                Some(last) => last.elapsed() >= self.refresh_interval,
                None => true,
            };
            if should_refresh {
                self.refresh();
            }
        }
        self.clamp_focus();

        if canvas.height() == 0 {
            self.dirty = false;
            return;
        }

        self.render_title(canvas);

        if canvas.height() < 2 {
            self.dirty = false;
            return;
        }

        if self.piece_count == 0 {
            canvas.print_at(0, 1, "[No pieces]");
            self.dirty = false;
            return;
        }

        if self.peers.is_empty() {
            let message = if self.download.is_some() {
                "[No peer availability]"
            } else {
                "[No peer availability snapshot]"
            };
            canvas.print_at(0, 1, message);
            self.dirty = false;
            return;
        }

        self.render_rows(canvas);
        self.dirty = false;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

/// Character-safe truncation/padding for the peer label.
fn fit_label(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let count = text.chars().count();
    let mut out: String = if count > width {
        let mut truncated: String = text.chars().take(width.saturating_sub(1)).collect();
        truncated.push('…');
        truncated
    } else {
        text.to_string()
    };
    let len = out.chars().count();
    if len < width {
        out.push_str(&" ".repeat(width - len));
    }
    out
}

/// Convenience constructor used by tests and callers that want an active
/// window without going through the root frame.
pub fn chunks_seen_window() -> ChunksSeenWindow {
    let mut window = ChunksSeenWindow::new();
    window.flags = crate::display::FLAG_ACTIVE;
    window
}
