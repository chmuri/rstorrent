//! Transfer-list window.
//!
//! Shows the outstanding block transfers owned by the focused download's
//! [`Delegator`]. Rows are detached [`TransferRow`] values so navigation and
//! drawing never hold an engine lock. [`TransferListWindow::refresh`] rebuilds
//! the snapshot from public `lt-engine` accessors; the `lt-storage` transfer
//! types are reached through those accessors without being named here.
//!
//! Active rows are filtered as the snapshot is taken: finished blocks and
//! erased transfer entries never become rows. When rows disappear between
//! refreshes the selection is restored by a semantic key and then clamped, so
//! stale indices cannot point at another peer's transfer.

use crate::display::{
    Canvas, TextElement, TextElementList, TextElementString, Window, EXTENT_FULL,
};
use rt_core::Download;
use rt_rpc::Target;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// Rows reserved for the title and column header.
const LIST_HEADER_ROWS: u32 = 2;

/// Visual state of one block transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransferState {
    #[default]
    Queued,
    Downloading,
    Completed,
}

impl TransferState {
    pub fn label(self) -> &'static str {
        match self {
            TransferState::Queued => "queued",
            TransferState::Downloading => "downloading",
            TransferState::Completed => "completed",
        }
    }

    pub fn is_active(self) -> bool {
        self != TransferState::Completed
    }
}

/// A detached snapshot of one active block transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferRow {
    /// Piece index the block belongs to.
    pub piece_index: u32,
    /// Block index inside that piece.
    pub block_index: u32,
    /// Byte offset of the block inside the piece.
    pub block_offset: u32,
    /// Block length in bytes.
    pub block_length: u32,
    /// Engine peer number, derived from the peer id by the engine.
    pub peer_id: usize,
    /// Best-effort peer address; falls back to `#<peer_id>`.
    pub peer_address: String,
    pub state: TransferState,
    pub is_leader: bool,
    pub is_stalled: bool,
    pub is_erased: bool,
    /// Completed blocks in the owning piece.
    pub piece_blocks_done: usize,
    /// Total blocks in the owning piece.
    pub piece_blocks_total: usize,
}

impl TransferRow {
    pub fn new(piece_index: u32, block_offset: u32, block_length: u32, peer_id: usize) -> Self {
        Self {
            piece_index,
            block_index: 0,
            block_offset,
            block_length,
            peer_id,
            peer_address: format!("#{}", peer_id),
            state: TransferState::Queued,
            is_leader: false,
            is_stalled: false,
            is_erased: false,
            piece_blocks_done: 0,
            piece_blocks_total: 0,
        }
    }

    pub fn with_block_index(mut self, block_index: u32) -> Self {
        self.block_index = block_index;
        self
    }

    pub fn with_peer_address(mut self, address: impl Into<String>) -> Self {
        self.peer_address = address.into();
        self
    }

    pub fn with_state(mut self, state: TransferState) -> Self {
        self.state = state;
        self
    }

    pub fn with_progress(mut self, done: usize, total: usize) -> Self {
        self.piece_blocks_done = done;
        self.piece_blocks_total = total;
        self
    }

    pub fn with_flags(mut self, is_leader: bool, is_stalled: bool, is_erased: bool) -> Self {
        self.is_leader = is_leader;
        self.is_stalled = is_stalled;
        self.is_erased = is_erased;
        self
    }

    /// Stable identity used to preserve selection across refreshes.
    pub fn key(&self) -> (u32, u32, usize) {
        (self.piece_index, self.block_offset, self.peer_id)
    }

    pub fn progress_percent(&self) -> u32 {
        if self.piece_blocks_total == 0 {
            0
        } else {
            ((self.piece_blocks_done.min(self.piece_blocks_total) as u128 * 100)
                / self.piece_blocks_total as u128) as u32
        }
    }

    pub fn progress_label(&self) -> String {
        if self.piece_blocks_total == 0 {
            "-".to_string()
        } else {
            format!(
                "{}/{} {:>3}%",
                self.piece_blocks_done,
                self.piece_blocks_total,
                self.progress_percent()
            )
        }
    }

    /// Column cells (without the focus marker), against explicit widths.
    /// A zero width hides that column.
    pub fn cells(&self, widths: &TransferColumnWidths) -> Vec<String> {
        let mut cells = Vec::with_capacity(7);
        push_cell(
            &mut cells,
            &self.piece_index.to_string(),
            widths.piece,
            true,
        );
        push_cell(
            &mut cells,
            &format!("B{}", self.block_index),
            widths.block,
            true,
        );
        push_cell(
            &mut cells,
            &self.block_offset.to_string(),
            widths.offset,
            true,
        );
        push_cell(
            &mut cells,
            &self.block_length.to_string(),
            widths.length,
            true,
        );
        push_cell(&mut cells, &self.peer_address, widths.peer, false);
        push_cell(&mut cells, self.state.label(), widths.state, false);
        push_cell(&mut cells, &self.progress_label(), widths.progress, false);
        cells
    }

    /// One complete row including the focus marker.
    pub fn render_line(&self, widths: &TransferColumnWidths, focused: bool) -> String {
        let marker = if focused { "* " } else { "  " };
        join_cells(marker, &self.cells(widths))
    }
}

/// Configurable widths for the transfer list columns. Zero hides a column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferColumnWidths {
    pub piece: usize,
    pub block: usize,
    pub offset: usize,
    pub length: usize,
    pub peer: usize,
    pub state: usize,
    pub progress: usize,
}

impl Default for TransferColumnWidths {
    fn default() -> Self {
        Self::full()
    }
}

impl TransferColumnWidths {
    pub const fn full() -> Self {
        Self {
            piece: 8,
            block: 6,
            offset: 9,
            length: 8,
            peer: 22,
            state: 11,
            progress: 10,
        }
    }

    pub fn for_width(width: usize) -> Self {
        if width >= 100 {
            Self::full()
        } else if width >= 80 {
            Self {
                piece: 7,
                block: 5,
                offset: 8,
                length: 7,
                peer: 18,
                state: 10,
                progress: 9,
            }
        } else if width >= 52 {
            Self {
                piece: 7,
                block: 0,
                offset: 8,
                length: 0,
                peer: 14,
                state: 9,
                progress: 8,
            }
        } else if width >= 40 {
            Self {
                piece: 6,
                block: 0,
                offset: 7,
                length: 0,
                peer: 9,
                state: 0,
                progress: 7,
            }
        } else if width >= 24 {
            Self {
                piece: 6,
                block: 0,
                offset: 0,
                length: 0,
                peer: 8,
                state: 0,
                progress: 6,
            }
        } else {
            Self {
                piece: 0,
                block: 0,
                offset: 0,
                length: 0,
                peer: 8,
                state: 0,
                progress: 0,
            }
        }
    }

    pub fn header_cells(&self) -> Vec<String> {
        let mut cells = Vec::with_capacity(7);
        push_cell(&mut cells, "Piece", self.piece, true);
        push_cell(&mut cells, "Blk", self.block, true);
        push_cell(&mut cells, "Offset", self.offset, true);
        push_cell(&mut cells, "Length", self.length, true);
        push_cell(&mut cells, "Peer", self.peer, false);
        push_cell(&mut cells, "State", self.state, false);
        push_cell(&mut cells, "Progress", self.progress, false);
        cells
    }

    pub fn header_line(&self) -> String {
        join_cells("  ", &self.header_cells())
    }
}

/// Snapshot of the live delegator used by [`TransferListWindow::refresh`].
struct TransferSnapshot {
    name: String,
    rows: Vec<TransferRow>,
}

/// Standalone transfer-list window.
pub struct TransferListWindow {
    pub flags: u32,
    pub download: Option<Arc<RwLock<Download>>>,
    pub download_name: String,
    pub transfers: Vec<TransferRow>,
    pub selected: usize,
    pub scroll_offset: usize,
    pub page_size: usize,
    /// `None` uses [`TransferColumnWidths::for_width`] at draw time.
    pub column_widths: Option<TransferColumnWidths>,
    /// When true, `redraw` re-snapshots the live delegator at the configured interval.
    pub auto_refresh: bool,
    /// Throttling interval for auto-refreshing the transfer list (default 1.0s).
    pub refresh_interval: std::time::Duration,
    last_refresh: Option<std::time::Instant>,
    dirty: bool,
}

impl Default for TransferListWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl TransferListWindow {
    pub fn new() -> Self {
        Self {
            flags: 0,
            download: None,
            download_name: String::new(),
            transfers: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            page_size: 10,
            column_widths: None,
            auto_refresh: true,
            refresh_interval: std::time::Duration::from_secs(1),
            last_refresh: None,
            dirty: true,
        }
    }

    /// Bind to a live download. A different pointer resets the selection.
    pub fn set_download(&mut self, download: Option<Arc<RwLock<Download>>>) {
        let changed = match (&self.download, &download) {
            (Some(old), Some(new)) => !Arc::ptr_eq(old, new),
            (None, None) => false,
            _ => true,
        };

        self.download = download;
        if changed {
            self.download_name.clear();
            self.transfers.clear();
            self.selected = 0;
            self.scroll_offset = 0;
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

    /// Install detached rows. Selection is preserved by transfer key when the
    /// same row still exists, otherwise it is clamped into the new range.
    pub fn set_transfers(&mut self, transfers: Vec<TransferRow>) {
        let previous = self.selected_transfer().map(TransferRow::key);
        self.transfers = transfers;
        self.restore_selection(previous);
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    /// Set the title name for detached snapshots/tests.
    pub fn set_download_name(&mut self, name: impl Into<String>) {
        self.download_name = name.into();
        self.dirty = true;
    }

    pub fn set_column_widths(&mut self, widths: Option<TransferColumnWidths>) {
        self.column_widths = widths;
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

    pub fn len(&self) -> usize {
        self.transfers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.transfers.is_empty()
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected_transfer(&self) -> Option<&TransferRow> {
        self.transfers.get(self.selected)
    }

    pub fn transfer_at(&self, index: usize) -> Option<&TransferRow> {
        self.transfers.get(index)
    }

    /// Re-snapshot the live delegator. Returns no rows when no download is
    /// bound. Selection is restored by [`TransferRow::key`] before clamping.
    pub fn refresh(&mut self) {
        self.last_refresh = Some(std::time::Instant::now());
        let previous = self.selected_transfer().map(TransferRow::key);
        match self.snapshot_live() {
            Some(snapshot) => {
                self.download_name = snapshot.name;
                self.transfers = snapshot.rows;
                self.restore_selection(previous);
                self.ensure_visible(self.page_size);
            }
            None => {
                self.download_name.clear();
                self.transfers.clear();
                self.selected = 0;
                self.scroll_offset = 0;
            }
        }
        self.dirty = true;
    }

    pub fn select_index(&mut self, index: usize) -> bool {
        if index >= self.transfers.len() {
            return false;
        }
        self.selected = index;
        self.ensure_visible(self.page_size);
        self.dirty = true;
        true
    }

    pub fn select_first(&mut self) {
        if !self.transfers.is_empty() {
            self.select_index(0);
        }
    }

    pub fn select_last(&mut self) {
        if !self.transfers.is_empty() {
            let last = self.transfers.len() - 1;
            self.select_index(last);
        }
    }

    pub fn select_next(&mut self) -> bool {
        if self.transfers.is_empty() || self.selected + 1 >= self.transfers.len() {
            return false;
        }
        self.selected += 1;
        self.ensure_visible(self.page_size);
        self.dirty = true;
        true
    }

    pub fn select_prev(&mut self) -> bool {
        if self.selected == 0 {
            return false;
        }
        self.selected -= 1;
        self.ensure_visible(self.page_size);
        self.dirty = true;
        true
    }

    pub fn page_down(&mut self) -> bool {
        if self.transfers.is_empty() {
            return false;
        }
        let target = (self.selected + self.page_size).min(self.transfers.len() - 1);
        if target == self.selected {
            return false;
        }
        self.selected = target;
        self.ensure_visible(self.page_size);
        self.dirty = true;
        true
    }

    pub fn page_up(&mut self) -> bool {
        let target = self.selected.saturating_sub(self.page_size);
        if target == self.selected {
            return false;
        }
        self.selected = target;
        self.ensure_visible(self.page_size);
        self.dirty = true;
        true
    }

    /// Keep `selected` inside the visible window of `visible_rows` rows.
    pub fn ensure_visible(&mut self, visible_rows: usize) {
        self.normalize_selection();
        if visible_rows == 0 || self.transfers.is_empty() {
            self.scroll_offset = 0;
            return;
        }
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + visible_rows {
            self.scroll_offset = self.selected + 1 - visible_rows;
        }
        let max_offset = self.transfers.len().saturating_sub(visible_rows);
        self.scroll_offset = self.scroll_offset.min(max_offset);
    }

    fn normalize_selection(&mut self) {
        if self.transfers.is_empty() {
            self.selected = 0;
            self.scroll_offset = 0;
        } else if self.selected >= self.transfers.len() {
            self.selected = self.transfers.len() - 1;
        }
    }

    fn restore_selection(&mut self, previous: Option<(u32, u32, usize)>) {
        if let Some(key) = previous {
            if let Some(position) = self.transfers.iter().position(|row| row.key() == key) {
                self.selected = position;
            }
        }
        self.normalize_selection();
    }

    fn effective_widths(&self, width: u32) -> TransferColumnWidths {
        self.column_widths
            .unwrap_or_else(|| TransferColumnWidths::for_width(width as usize))
    }

    /// Take a lock-free snapshot of the download's active block transfers.
    ///
    /// The engine identifies delegator peers by a numeric key derived from the
    /// first two peer-id bytes (`Download::peer_number`). The same derivation
    /// is used here to attach a best-effort address from the live connection
    /// list; a missing connection simply leaves `#<peer_id>` as the label.
    fn snapshot_live(&self) -> Option<TransferSnapshot> {
        let download = self.download.as_ref()?;
        let guard = download.read().ok()?;
        let engine = guard.download();

        let mut peer_addresses: HashMap<usize, String> = HashMap::new();
        for connection in engine.connection_list().iter() {
            let bytes = connection.peer_id.as_bytes();
            let peer_number = ((bytes[0] as usize) << 8) | bytes[1] as usize;
            peer_addresses
                .entry(peer_number)
                .or_insert_with(|| connection.socket_address.pretty_str());
        }

        let mut rows = Vec::new();
        for block_list in engine.delegator().transfers().iter() {
            let piece_done = block_list.finished();
            let piece_total = block_list.len();

            for (block_index, block) in block_list.blocks().iter().enumerate() {
                // Completed blocks are removed from the active view.
                if block.is_finished() {
                    continue;
                }

                for transfer in block.transfers() {
                    // Erased entries are historical and must never render.
                    if transfer.is_erased() {
                        continue;
                    }

                    let piece = transfer.piece();
                    let peer_id = transfer.peer_id();
                    let state = if transfer.is_leader() {
                        TransferState::Downloading
                    } else {
                        TransferState::Queued
                    };

                    rows.push(TransferRow {
                        piece_index: piece.index(),
                        block_index: block_index as u32,
                        block_offset: piece.offset(),
                        block_length: piece.length(),
                        peer_id,
                        peer_address: peer_addresses
                            .get(&peer_id)
                            .cloned()
                            .unwrap_or_else(|| format!("#{}", peer_id)),
                        state,
                        is_leader: transfer.is_leader(),
                        is_stalled: transfer.is_stalled() || block.is_stalled(),
                        is_erased: false,
                        piece_blocks_done: piece_done,
                        piece_blocks_total: piece_total,
                    });
                }
            }
        }

        rows.sort_by_key(|row| (row.piece_index, row.block_offset, row.peer_id));

        Some(TransferSnapshot {
            name: guard.name().to_string(),
            rows,
        })
    }

    fn render_title(&self, canvas: &mut Canvas) {
        let name = if self.download_name.is_empty() {
            "?"
        } else {
            self.download_name.as_str()
        };
        let title = format!("[Transfers: {}] {} active", name, self.transfers.len());
        canvas.print_at(0, 0, &title);

        if self.transfers.is_empty() {
            return;
        }

        let counter = format!("[{:3} of {:<3}]", self.selected + 1, self.transfers.len());
        let used = title.len() + counter.len() + 2;
        if (canvas.width() as usize) > used {
            let x = canvas.width().saturating_sub(counter.len() as u32);
            canvas.print_at(x, 0, &counter);
        }
    }

    fn render_rows(&mut self, canvas: &mut Canvas) {
        if canvas.height() < LIST_HEADER_ROWS {
            return;
        }

        let widths = self.effective_widths(canvas.width());
        let header_style = ratatui::style::Style::default()
            .add_modifier(ratatui::style::Modifier::BOLD)
            .add_modifier(ratatui::style::Modifier::UNDERLINED);
        canvas.print_at_styled(0, 1, &widths.header_line(), header_style);

        let visible_rows = canvas.height().saturating_sub(LIST_HEADER_ROWS) as usize;
        if visible_rows == 0 {
            return;
        }

        self.ensure_visible(visible_rows);

        let end = (self.scroll_offset + visible_rows).min(self.transfers.len());
        for (offset, index) in (self.scroll_offset..end).enumerate() {
            let y = LIST_HEADER_ROWS + offset as u32;
            if y >= canvas.height() {
                break;
            }
            let row = &self.transfers[index];
            let focused = index == self.selected;
            let line = row.render_line(&widths, focused);
            let style = if focused {
                ratatui::style::Style::default()
                    .fg(ratatui::style::Color::Black)
                    .bg(ratatui::style::Color::White)
                    .add_modifier(ratatui::style::Modifier::BOLD)
            } else {
                ratatui::style::Style::default()
            };
            canvas.print_at_styled(0, y, &line, style);
        }
    }
}

impl Window for TransferListWindow {
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

        // A completely unbound window has a single, unambiguous empty state.
        if self.download.is_none() && self.download_name.is_empty() && self.transfers.is_empty() {
            canvas.print_at(0, 0, "[No download selected]");
            self.dirty = false;
            return;
        }

        self.render_title(canvas);

        if self.transfers.is_empty() {
            if canvas.height() > 1 {
                canvas.print_at(0, 1, "[No active transfers]");
            }
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

fn push_cell(cells: &mut Vec<String>, text: &str, width: usize, right_align: bool) {
    if width == 0 {
        return;
    }
    cells.push(fit(text, width, right_align));
}

/// Character-safe truncation with padding.
fn fit(text: &str, width: usize, right_align: bool) -> String {
    if width == 0 {
        return String::new();
    }
    let mut truncated: String = text.chars().take(width).collect();
    if truncated.chars().count() < text.chars().count() && width > 1 {
        truncated = text.chars().take(width - 1).collect();
        truncated.push('…');
    }
    let len = truncated.chars().count();
    if len < width {
        let padding = " ".repeat(width - len);
        if right_align {
            truncated.insert_str(0, &padding);
        } else {
            truncated.push_str(&padding);
        }
    }
    truncated
}

/// Compose cells through `TextElement` so the window follows the same element
/// model as the rest of the TUI.
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

/// Convenience constructor used by tests and callers that want an active
/// window without going through the root frame.
pub fn transfer_list_window() -> TransferListWindow {
    let mut window = TransferListWindow::new();
    window.flags = crate::display::FLAG_ACTIVE;
    window
}
