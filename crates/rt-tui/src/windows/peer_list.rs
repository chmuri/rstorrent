//! Peer list window.
//!
//! This window renders the peers attached to the currently selected download.
//! It deliberately keeps a small, value-based [`PeerRow`] snapshot so that
//! rendering and navigation do not need to hold any locks from `lt-protocol`
//! or `lt-engine` while the terminal is being drawn.
//!
//! The live [`rt_core::Download`] is only touched when taking a new snapshot
//! ([`PeerListWindow::refresh`]) or when applying a peer action.

use crate::display::{
    Canvas, TextElement, TextElementList, TextElementString, Window, EXTENT_FULL, FLAG_ACTIVE,
};
use lt_common::HashString;
use ratatui::style::{Color, Modifier, Style};
use rt_core::Download;
use rt_rpc::Target;
use std::sync::{Arc, RwLock};

/// Number of terminal rows reserved above the peer rows (title + columns).
const LIST_HEADER_ROWS: u32 = 2;

/// Which direction the connection was initiated from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PeerDirectionView {
    #[default]
    Outgoing,
    Incoming,
}

impl PeerDirectionView {
    pub fn as_str(self) -> &'static str {
        match self {
            PeerDirectionView::Outgoing => "out",
            PeerDirectionView::Incoming => "in",
        }
    }
}

/// A detached snapshot of one peer, suitable for rendering and for stale-safe
/// actions (the `peer_id` is used to find the live connection again).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PeerRow {
    pub peer_id: HashString,
    pub address: String,
    pub client: String,
    pub direction: PeerDirectionView,
    pub encrypted: bool,
    pub obfuscated: bool,
    pub snubbed: bool,
    pub banned: bool,
    pub preferred: bool,
    pub seed: bool,
    /// Bytes per second, matching the download list's KB/s rendering.
    pub rate_up: u64,
    /// Bytes per second, matching the download list's KB/s rendering.
    pub rate_down: u64,
    pub uploaded: u64,
    pub downloaded: u64,
}

impl PeerRow {
    pub fn new(peer_id: HashString, address: impl Into<String>, client: impl Into<String>) -> Self {
        Self {
            peer_id,
            address: address.into(),
            client: client.into(),
            ..Self::default()
        }
    }

    pub fn with_rates(mut self, rate_up: u64, rate_down: u64) -> Self {
        self.rate_up = rate_up;
        self.rate_down = rate_down;
        self
    }

    pub fn with_totals(mut self, uploaded: u64, downloaded: u64) -> Self {
        self.uploaded = uploaded;
        self.downloaded = downloaded;
        self
    }

    pub fn with_direction(mut self, direction: PeerDirectionView) -> Self {
        self.direction = direction;
        self
    }

    /// Set all boolean peer flags in one call.
    #[allow(clippy::too_many_arguments)]
    pub fn with_flags(
        mut self,
        encrypted: bool,
        obfuscated: bool,
        snubbed: bool,
        preferred: bool,
        banned: bool,
        seed: bool,
    ) -> Self {
        self.encrypted = encrypted;
        self.obfuscated = obfuscated;
        self.snubbed = snubbed;
        self.preferred = preferred;
        self.banned = banned;
        self.seed = seed;
        self
    }

    pub fn is_incoming(&self) -> bool {
        self.direction == PeerDirectionView::Incoming
    }

    /// Compact flag column. Encryption and obfuscation are distinct letters so
    /// the two states remain distinguishable, as old rTorrent does.
    ///
    /// `E` encrypted, `O` obfuscated, `I` incoming, `N` snubbed,
    /// `P` preferred, `B` banned, `S` seeder.
    pub fn flags_string(&self) -> String {
        let mut flags = String::new();
        if self.encrypted {
            flags.push('E');
        }
        if self.obfuscated {
            flags.push('O');
        }
        if self.is_incoming() {
            flags.push('I');
        }
        if self.snubbed {
            flags.push('N');
        }
        if self.preferred {
            flags.push('P');
        }
        if self.banned {
            flags.push('B');
        }
        if self.seed {
            flags.push('S');
        }
        if flags.is_empty() {
            flags.push('-');
        }
        flags
    }

    /// Column cells (without the leading focus marker), against explicit
    /// widths. A width of zero hides that column.
    pub fn cells(&self, widths: &PeerColumnWidths) -> Vec<String> {
        let mut cells = Vec::with_capacity(8);
        push_cell(&mut cells, &self.address, widths.address, false);
        push_cell(&mut cells, &self.client, widths.client, false);
        push_cell(&mut cells, self.direction.as_str(), widths.direction, false);
        push_cell(&mut cells, &format_rate(self.rate_up), widths.rate_up, true);
        push_cell(
            &mut cells,
            &format_rate(self.rate_down),
            widths.rate_down,
            true,
        );
        push_cell(
            &mut cells,
            &format_totals(self.uploaded),
            widths.uploaded,
            true,
        );
        push_cell(
            &mut cells,
            &format_totals(self.downloaded),
            widths.downloaded,
            true,
        );
        push_cell(&mut cells, &self.flags_string(), widths.flags, false);
        cells
    }

    /// Convenience rendering of one row, including the focus marker for the
    /// currently selected peer.
    pub fn render_line(&self, widths: &PeerColumnWidths, focused: bool) -> String {
        let marker = if focused { "* " } else { "  " };
        join_cells(marker, &self.cells(widths))
    }
}

/// Configurable widths for the peer list columns. A zero width hides a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerColumnWidths {
    pub address: usize,
    pub client: usize,
    pub direction: usize,
    pub rate_up: usize,
    pub rate_down: usize,
    pub uploaded: usize,
    pub downloaded: usize,
    pub flags: usize,
}

impl Default for PeerColumnWidths {
    fn default() -> Self {
        Self::full()
    }
}

impl PeerColumnWidths {
    /// Widest layout, used for wide terminals.
    pub const fn full() -> Self {
        Self {
            address: 22,
            client: 20,
            direction: 3,
            rate_up: 9,
            rate_down: 9,
            uploaded: 10,
            downloaded: 10,
            flags: 9,
        }
    }

    /// Pick a layout that reasonably fits `width` terminal columns.
    pub fn for_width(width: usize) -> Self {
        if width >= 100 {
            Self::full()
        } else if width >= 82 {
            Self {
                address: 18,
                client: 14,
                direction: 3,
                rate_up: 8,
                rate_down: 8,
                uploaded: 8,
                downloaded: 8,
                flags: 7,
            }
        } else if width >= 62 {
            Self {
                address: 18,
                client: 12,
                direction: 3,
                rate_up: 8,
                rate_down: 8,
                uploaded: 0,
                downloaded: 0,
                flags: 7,
            }
        } else if width >= 48 {
            Self {
                address: 18,
                client: 0,
                direction: 3,
                rate_up: 8,
                rate_down: 8,
                uploaded: 0,
                downloaded: 0,
                flags: 7,
            }
        } else if width >= 32 {
            Self {
                address: width.saturating_sub(20).max(8),
                client: 0,
                direction: 0,
                rate_up: 7,
                rate_down: 7,
                uploaded: 0,
                downloaded: 0,
                flags: 3,
            }
        } else {
            Self {
                address: width.saturating_sub(1).max(4),
                client: 0,
                direction: 0,
                rate_up: 0,
                rate_down: 0,
                uploaded: 0,
                downloaded: 0,
                flags: 0,
            }
        }
    }

    pub fn header_cells(&self) -> Vec<String> {
        let mut cells = Vec::with_capacity(8);
        push_cell(&mut cells, "Address", self.address, false);
        push_cell(&mut cells, "Client", self.client, false);
        push_cell(&mut cells, "Dir", self.direction, false);
        push_cell(&mut cells, "Up KB/s", self.rate_up, true);
        push_cell(&mut cells, "Down KB/s", self.rate_down, true);
        push_cell(&mut cells, "Uploaded", self.uploaded, true);
        push_cell(&mut cells, "Downloaded", self.downloaded, true);
        push_cell(&mut cells, "Flags", self.flags, false);
        cells
    }

    /// Header line with the standard leading marker indentation.
    pub fn header_line(&self) -> String {
        join_cells("  ", &self.header_cells())
    }
}

/// Stable ordering choices for the peer list. `ConnectionOrder` preserves the
/// order reported by `ConnectionList`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PeerSort {
    #[default]
    ConnectionOrder,
    RateDown,
    RateUp,
    Address,
}

/// Actions that can be applied to the selected peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerAction {
    Disconnect,
    ToggleSnub,
    TogglePrefer,
    ToggleBan,
}

/// Result of attempting a peer action. Stale selections are expected: peers
/// may disappear between rendering and the action itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerActionOutcome {
    Applied,
    NoSelection,
    NoDownload,
    PeerGone,
    Unavailable,
}

impl PeerActionOutcome {
    pub fn is_applied(self) -> bool {
        self == PeerActionOutcome::Applied
    }
}

/// Standalone peer-list window.
pub struct PeerListWindow {
    pub flags: u32,
    pub download: Option<Arc<RwLock<Download>>>,
    pub peers: Vec<PeerRow>,
    pub selected: usize,
    pub scroll_offset: usize,
    pub page_size: usize,
    /// `None` means use [`PeerColumnWidths::for_width`] at draw time.
    pub column_widths: Option<PeerColumnWidths>,
    pub sort_mode: PeerSort,
    /// When true, `redraw` snapshots the live download at the configured interval.
    pub auto_refresh: bool,
    /// Throttling interval for auto-refreshing the peer list (permanently throttled to 1.0s by default).
    pub refresh_interval: std::time::Duration,
    last_refresh: Option<std::time::Instant>,
    dirty: bool,
}

impl Default for PeerListWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl PeerListWindow {
    pub fn new() -> Self {
        Self {
            flags: 0,
            download: None,
            peers: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            page_size: 10,
            column_widths: None,
            sort_mode: PeerSort::ConnectionOrder,
            auto_refresh: true,
            refresh_interval: std::time::Duration::from_secs(1),
            last_refresh: None,
            dirty: true,
        }
    }

    pub fn set_download(&mut self, download: Option<Arc<RwLock<Download>>>) {
        self.download = download;
        self.selected = 0;
        self.scroll_offset = 0;
        self.last_refresh = None;
        self.refresh();
    }

    /// Set throttling interval for auto-refreshing the peer list.
    pub fn set_refresh_interval(&mut self, interval: std::time::Duration) {
        self.refresh_interval = interval;
    }

    /// Invalidate the refresh timestamp to force a refresh on the next redraw.
    pub fn invalidate_refresh(&mut self) {
        self.last_refresh = None;
        self.dirty = true;
    }

    /// Install a detached snapshot, primarily useful for tests and for
    /// callers that already have their own peer view model.
    pub fn set_peers(&mut self, peers: Vec<PeerRow>) {
        self.peers = peers;
        self.normalize_selection();
        self.scroll_offset = 0;
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    pub fn set_column_widths(&mut self, widths: Option<PeerColumnWidths>) {
        self.column_widths = widths;
        self.dirty = true;
    }

    pub fn set_sort_mode(&mut self, mode: PeerSort) {
        let previous = self.selection_key();
        self.sort_mode = mode;
        self.apply_sort();
        self.restore_selection(previous);
        self.dirty = true;
    }

    pub fn set_page_size(&mut self, page_size: usize) {
        self.page_size = page_size.max(1);
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    pub fn set_auto_refresh(&mut self, auto_refresh: bool) {
        self.auto_refresh = auto_refresh;
        self.dirty = true;
    }

    /// Map the peer-list action keys to actions. The orchestrator should route
    /// these before the global throttle keys while the peer list is focused:
    /// `d` disconnect, `s`/`*` snub, `p` prefer, `b`/`B` ban. The `*` and
    /// `B` aliases match the old rTorrent peer-list bindings.
    pub fn action_for_key(key: i32) -> Option<PeerAction> {
        match key {
            k if k == b'd' as i32 => Some(PeerAction::Disconnect),
            k if k == b's' as i32 || k == b'*' as i32 => Some(PeerAction::ToggleSnub),
            k if k == b'p' as i32 => Some(PeerAction::TogglePrefer),
            k if k == b'b' as i32 || k == b'B' as i32 => Some(PeerAction::ToggleBan),
            _ => None,
        }
    }

    /// Handle a peer action key, if the key maps to one. Returns `None` for
    /// unrelated keys so the caller can continue normal key processing.
    pub fn handle_action_key(&mut self, key: i32) -> Option<PeerActionOutcome> {
        let action = Self::action_for_key(key)?;
        Some(self.apply_selected_action(action))
    }

    pub fn len(&self) -> usize {
        self.peers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected_peer(&self) -> Option<&PeerRow> {
        self.peers.get(self.selected)
    }

    pub fn peer_at(&self, index: usize) -> Option<&PeerRow> {
        self.peers.get(index)
    }

    /// Re-snapshot the live download's peer list. Selection is preserved by
    /// peer id + address; if that peer vanished the index is clamped.
    pub fn refresh(&mut self) {
        self.last_refresh = Some(std::time::Instant::now());
        let previous = self.selection_key();
        match self.snapshot_live_peers() {
            Some(peers) => {
                self.peers = peers;
                self.apply_sort();
                self.restore_selection(previous);
                self.ensure_visible(self.page_size);
            }
            None => {
                self.peers.clear();
                self.selected = 0;
                self.scroll_offset = 0;
            }
        }
        self.dirty = true;
    }

    pub fn select_index(&mut self, index: usize) -> bool {
        if self.peers.is_empty() {
            self.selected = 0;
            return false;
        }
        if index >= self.peers.len() {
            return false;
        }
        self.selected = index;
        self.ensure_visible(self.page_size);
        self.dirty = true;
        true
    }

    pub fn select_first(&mut self) {
        if !self.peers.is_empty() {
            self.select_index(0);
        }
    }

    pub fn select_last(&mut self) {
        if !self.peers.is_empty() {
            let last = self.peers.len() - 1;
            self.select_index(last);
        }
    }

    /// Move down one row, wrapping at the end.
    pub fn select_next(&mut self) {
        self.move_selection(1);
    }

    /// Move up one row, wrapping at the start.
    pub fn select_prev(&mut self) {
        self.move_selection(-1);
    }

    pub fn page_down(&mut self) {
        if self.peers.is_empty() {
            return;
        }
        let page = self.page_size.max(1);
        self.selected = (self.selected + page).min(self.peers.len() - 1);
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    pub fn page_up(&mut self) {
        if self.peers.is_empty() {
            return;
        }
        let page = self.page_size.max(1);
        self.selected = self.selected.saturating_sub(page);
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    /// Keep `selected` inside the visible window of `visible_rows` rows.
    pub fn ensure_visible(&mut self, visible_rows: usize) {
        if visible_rows == 0 || self.peers.is_empty() {
            return;
        }
        self.normalize_selection();
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + visible_rows {
            self.scroll_offset = self.selected + 1 - visible_rows;
        }
        let max_offset = self.peers.len().saturating_sub(visible_rows);
        self.scroll_offset = self.scroll_offset.min(max_offset);
    }

    /// Apply an action to the currently selected peer. The live connection is
    /// looked up again by peer id, so stale row indices cannot disconnect the
    /// wrong peer.
    pub fn apply_selected_action(&mut self, action: PeerAction) -> PeerActionOutcome {
        let peer = match self.selected_peer() {
            Some(peer) => peer.clone(),
            None => return PeerActionOutcome::NoSelection,
        };
        let download = match &self.download {
            Some(download) => Arc::clone(download),
            None => return PeerActionOutcome::NoDownload,
        };

        let outcome = {
            let mut guard = match download.write() {
                Ok(guard) => guard,
                Err(_) => return PeerActionOutcome::Unavailable,
            };
            let mut engine = guard.download_mut();
            let list = engine.connection_list_mut();

            match action {
                PeerAction::Disconnect => match list.erase(Some(&peer.peer_id), 0) {
                    Ok(()) => PeerActionOutcome::Applied,
                    Err(_) => PeerActionOutcome::PeerGone,
                },
                PeerAction::ToggleSnub | PeerAction::TogglePrefer | PeerAction::ToggleBan => {
                    match list.find_by_id_mut(&peer.peer_id) {
                        Some(connection) => {
                            match action {
                                PeerAction::ToggleSnub => {
                                    connection.is_snubbed = !connection.is_snubbed;
                                }
                                PeerAction::TogglePrefer => {
                                    connection.is_preferred = !connection.is_preferred;
                                }
                                PeerAction::ToggleBan => {
                                    connection.is_banned = !connection.is_banned;
                                }
                                PeerAction::Disconnect => unreachable!(),
                            }
                            PeerActionOutcome::Applied
                        }
                        None => PeerActionOutcome::PeerGone,
                    }
                }
            }
        };

        if outcome.is_applied() {
            self.refresh();
        }
        outcome
    }

    pub fn disconnect_selected(&mut self) -> PeerActionOutcome {
        self.apply_selected_action(PeerAction::Disconnect)
    }

    pub fn toggle_snub_selected(&mut self) -> PeerActionOutcome {
        self.apply_selected_action(PeerAction::ToggleSnub)
    }

    pub fn toggle_prefer_selected(&mut self) -> PeerActionOutcome {
        self.apply_selected_action(PeerAction::TogglePrefer)
    }

    pub fn toggle_ban_selected(&mut self) -> PeerActionOutcome {
        self.apply_selected_action(PeerAction::ToggleBan)
    }

    fn selection_key(&self) -> Option<(HashString, String)> {
        self.selected_peer()
            .map(|peer| (peer.peer_id, peer.address.clone()))
    }

    fn restore_selection(&mut self, previous: Option<(HashString, String)>) {
        if let Some((peer_id, address)) = previous {
            if let Some(pos) = self
                .peers
                .iter()
                .position(|peer| peer.peer_id == peer_id && peer.address == address)
            {
                self.selected = pos;
            }
        }
        self.normalize_selection();
    }

    fn normalize_selection(&mut self) {
        if self.peers.is_empty() {
            self.selected = 0;
            self.scroll_offset = 0;
        } else if self.selected >= self.peers.len() {
            self.selected = self.peers.len() - 1;
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.peers.is_empty() {
            return;
        }
        let len = self.peers.len() as isize;
        let current = self.selected.min(self.peers.len() - 1) as isize;
        self.selected = (current + delta).rem_euclid(len) as usize;
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    fn apply_sort(&mut self) {
        match self.sort_mode {
            PeerSort::ConnectionOrder => {}
            PeerSort::RateDown => self
                .peers
                .sort_by_key(|peer| std::cmp::Reverse(peer.rate_down)),
            PeerSort::RateUp => self
                .peers
                .sort_by_key(|peer| std::cmp::Reverse(peer.rate_up)),
            PeerSort::Address => self.peers.sort_by(|a, b| a.address.cmp(&b.address)),
        }
    }

    /// Take a lock-free snapshot of the live download's connections.
    ///
    /// The peer connection type is intentionally not named here: this keeps
    /// `rt-tui` from needing a direct `lt-protocol` dependency.
    fn snapshot_live_peers(&self) -> Option<Vec<PeerRow>> {
        let download = self.download.as_ref()?;
        let download = download.read().ok()?;
        let engine = download.download();
        let peers = engine
            .connection_list()
            .iter()
            .map(|peer| {
                // `Rate::rate` takes `&mut self`; cloning gives a stable
                // snapshot for this frame without mutating the live conn.
                let mut rate_up = peer.rate_up.clone();
                let mut rate_down = peer.rate_down.clone();
                PeerRow {
                    peer_id: peer.peer_id,
                    address: peer.socket_address.pretty_str(),
                    client: peer.client_version(),
                    direction: if peer.is_incoming() {
                        PeerDirectionView::Incoming
                    } else {
                        PeerDirectionView::Outgoing
                    },
                    encrypted: peer.is_encrypted,
                    obfuscated: peer.is_obfuscated,
                    snubbed: peer.is_snubbed,
                    banned: peer.is_banned,
                    preferred: peer.is_preferred,
                    seed: peer.is_seeder(),
                    rate_up: rate_up.rate(),
                    rate_down: rate_down.rate(),
                    uploaded: peer.uploaded,
                    downloaded: peer.downloaded,
                }
            })
            .collect();
        Some(peers)
    }

    fn effective_widths(&self, width: u32) -> PeerColumnWidths {
        self.column_widths
            .unwrap_or_else(|| PeerColumnWidths::for_width(width as usize))
    }

    fn render_peers(&mut self, canvas: &mut Canvas, widths: &PeerColumnWidths) {
        let height = canvas.height();
        let header_style = Style::default()
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::UNDERLINED);
        canvas.print_at_styled(0, 1, &widths.header_line(), header_style);

        let visible_rows = height.saturating_sub(LIST_HEADER_ROWS) as usize;
        if visible_rows == 0 {
            return;
        }

        self.ensure_visible(visible_rows);

        let end = (self.scroll_offset + visible_rows).min(self.peers.len());
        for (row_offset, peer) in self.peers[self.scroll_offset..end].iter().enumerate() {
            let index = self.scroll_offset + row_offset;
            let focused = index == self.selected;
            let line = peer.render_line(widths, focused);
            let style = if focused {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if peer.banned {
                Style::default().fg(Color::Red)
            } else {
                Style::default()
            };
            canvas.print_at_styled(0, LIST_HEADER_ROWS + row_offset as u32, &line, style);
        }
    }
}

impl Window for PeerListWindow {
    fn flags(&self) -> u32 {
        self.flags
    }

    fn set_flags(&mut self, flags: u32) {
        self.flags = flags;
    }

    fn min_width(&self) -> u32 {
        16
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

    fn resize(&mut self, _x: u32, _y: u32, _width: u32, height: u32) {
        self.page_size = height.saturating_sub(LIST_HEADER_ROWS).max(1) as usize;
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    fn redraw(&mut self, canvas: &mut Canvas) {
        if canvas.is_daemon() {
            return;
        }
        canvas.erase();

        if self.auto_refresh && self.download.is_some() {
            let should_refresh = match self.last_refresh {
                Some(last) => last.elapsed() >= self.refresh_interval,
                None => true,
            };
            if should_refresh {
                self.refresh();
            }
        }
        self.normalize_selection();

        if canvas.height() == 0 {
            self.dirty = false;
            return;
        }

        let count = self.peers.len();
        let title = format!("[Peers: {}]", count);
        canvas.print_at(0, 0, &title);

        if count > 0 && canvas.width() > title.len() as u32 + 12 {
            let counter = format!("[{:3} of {:<3}]", self.selected + 1, count);
            let x = canvas.width().saturating_sub(counter.len() as u32);
            canvas.print_at(x, 0, &counter);
        }

        // Very small terminals still show a useful single line.
        if canvas.height() < 2 {
            self.dirty = false;
            return;
        }

        if count == 0 {
            let message = if self.download.is_some() {
                "[No peers connected]"
            } else {
                "[No download selected]"
            };
            canvas.print_at(0, 1, message);
            self.dirty = false;
            return;
        }

        let widths = self.effective_widths(canvas.width());
        self.render_peers(canvas, &widths);
        self.dirty = false;
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

fn format_rate(bytes_per_second: u64) -> String {
    format!("{:5.1}", bytes_per_second as f64 / 1024.0)
}

fn format_totals(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let value = bytes as f64;
    if value >= GB {
        format!("{:.1}G", value / GB)
    } else if value >= MB {
        format!("{:.1}M", value / MB)
    } else if value >= KB {
        format!("{:.1}K", value / KB)
    } else {
        format!("{}B", bytes)
    }
}

fn push_cell(cells: &mut Vec<String>, text: &str, width: usize, right_align: bool) {
    if width == 0 {
        return;
    }
    cells.push(fit(text, width, right_align));
}

fn fit(text: &str, width: usize, right_align: bool) -> String {
    let truncated: String = text.chars().take(width).collect();
    if right_align {
        format!("{:>width$}", truncated, width = width)
    } else {
        format!("{:<width$}", truncated, width = width)
    }
}

/// Compose cells through `TextElement` primitives so the window follows the
/// same element model as the rest of the TUI.
fn join_cells(prefix: &str, cells: &[String]) -> String {
    let mut list = TextElementList::new();
    list.push(Box::new(TextElementString::new(prefix.to_string())));
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            list.push(Box::new(TextElementString::new(" ".to_string())));
        }
        list.push(Box::new(TextElementString::new(cell.clone())));
    }
    list.render(&Target::None, None)
}

/// Convenience constructor used by integration tests and callers that want an
/// active window without going through the root frame.
pub fn peer_list_window() -> PeerListWindow {
    let mut window = PeerListWindow::new();
    window.flags = FLAG_ACTIVE;
    window
}
