//! Tracker list window.
//!
//! One row per tracker of the currently focused download. Rendering works on a
//! detached [`TrackerRow`] snapshot so the terminal is never drawn while the
//! engine or tracker controller lock is held.
//!
//! Tracker state is not re-exported by `rt-core`, and `lt-tracker` is not a
//! direct dependency of this crate. The snapshot adapter below therefore only
//! uses the public methods/fields reachable through
//! `Download::download().tracker_controller()`. This keeps `rt-tui` free of a
//! direct `lt-tracker` dependency; if the tracker API grows new fields they
//! only need to be added to [`TrackerRow`] and the snapshot adapter.
//!
//! Enable/disable actions go through the canonical `t.enable`/`t.disable`
//! commands. There are no `d.tracker_announce`/`d.tracker_scrape` commands in
//! `rt-core`, so announce/scrape use the `TrackerController`/`TrackerList`
//! APIs directly (the same calls those commands would make).
//!
//! DHT is not synthesized as a tracker row: only entries actually present in
//! the download's `TrackerList` are shown. A `dht://` URL already present in a
//! torrent is rendered with the `DHT` type instead of being treated as HTTP.

use crate::display::{
    Canvas, TextElement, TextElementList, TextElementString, Window, EXTENT_FULL, FLAG_ACTIVE,
};
use lt_common::Object;
use ratatui::style::{Color, Modifier, Style};
use rt_core::commands::tracker::{register_tracker_commands, DhtConfig};
use rt_core::{Download, DownloadList};
use rt_rpc::{CommandMap, Target};
use std::sync::{Arc, RwLock};

/// Rows reserved for the title and the column header.
const LIST_HEADER_ROWS: u32 = 2;

/// Tracker protocol family as exposed to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TrackerTypeView {
    #[default]
    Http,
    Udp,
    Dht,
}

impl TrackerTypeView {
    /// Derive the type from the URL, matching `lt_tracker::Tracker::new`.
    pub fn from_url(url: &str) -> Self {
        if url.starts_with("udp://") {
            TrackerTypeView::Udp
        } else if url.starts_with("dht://") {
            TrackerTypeView::Dht
        } else {
            TrackerTypeView::Http
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TrackerTypeView::Http => "HTTP",
            TrackerTypeView::Udp => "UDP",
            TrackerTypeView::Dht => "DHT",
        }
    }
}

/// Detached snapshot of one tracker.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrackerRow {
    /// Index in the download's `TrackerList`; this is what `t.*` targets use.
    pub index: usize,
    /// Position inside the tracker's tier/group.
    pub group_index: usize,
    pub group: u32,
    pub url: String,
    pub tracker_type: TrackerTypeView,
    pub enabled: bool,
    pub usable: bool,
    /// A request (announce or scrape) is currently in flight.
    pub busy: bool,
    pub in_use: bool,
    pub extra_tracker: bool,
    pub scrapable: bool,
    /// Numeric `lt_tracker::TrackerEvent` (0 none, 1 completed, 2 started,
    /// 3 stopped, 4 scrape). Kept numeric so this crate does not need the
    /// `lt-tracker` enum.
    pub latest_event: i64,
    pub normal_interval: u64,
    pub min_interval: u64,
    pub success_counter: u32,
    pub failed_counter: u32,
    pub scrape_counter: u32,
    /// Seconds until the next announce, when one is scheduled.
    pub next_announce_in: Option<u64>,
    pub latest_new_peers: u32,
    pub latest_sum_peers: u32,
    pub scrape_complete: u32,
    pub scrape_incomplete: u32,
    pub scrape_downloaded: u32,
}

impl TrackerRow {
    /// Construct an enabled, usable HTTP/UDP row from a URL.
    pub fn new(index: usize, group: u32, url: impl Into<String>) -> Self {
        let url = url.into();
        Self {
            index,
            group,
            tracker_type: TrackerTypeView::from_url(&url),
            url,
            enabled: true,
            usable: true,
            scrapable: true,
            normal_interval: 1800,
            min_interval: 600,
            ..Self::default()
        }
    }

    /// Compact state cell: `on`, `off`, `err`, `req` or `scr`.
    pub fn state_label(&self) -> &'static str {
        if self.busy {
            if self.latest_event == 4 {
                "scr"
            } else {
                "req"
            }
        } else if !self.enabled {
            "off"
        } else if !self.usable {
            "err"
        } else {
            "on"
        }
    }

    /// Human readable last tracker event.
    pub fn event_label(&self) -> &'static str {
        match self.latest_event {
            0 => "none",
            1 => "completed",
            2 => "started",
            3 => "stopped",
            4 => "scrape",
            _ => "?",
        }
    }

    /// Column cells (without the leading focus marker), against explicit
    /// widths. A width of zero hides that column.
    pub fn cells(&self, widths: &TrackerColumnWidths) -> Vec<String> {
        let mut cells = Vec::with_capacity(8);
        let group = if self.group_index == 0 {
            format!("{:>2}:{}", self.group, self.group_index)
        } else {
            format!("  :{}", self.group_index)
        };
        push_cell(&mut cells, &group, widths.group, false);
        push_cell_url(&mut cells, &self.url, widths.url);
        push_cell(
            &mut cells,
            self.tracker_type.as_str(),
            widths.tracker_type,
            false,
        );
        push_cell(&mut cells, self.state_label(), widths.state, false);
        push_cell(&mut cells, self.event_label(), widths.event, false);
        let next = match self.next_announce_in {
            Some(seconds) => format_next(seconds),
            None => "-".to_string(),
        };
        push_cell(&mut cells, &next, widths.next, true);
        let scrape = if self.scrape_counter == 0
            && self.scrape_complete == 0
            && self.scrape_incomplete == 0
            && self.scrape_downloaded == 0
        {
            "-".to_string()
        } else {
            format!(
                "{}/{}/{}",
                self.scrape_complete, self.scrape_incomplete, self.scrape_downloaded
            )
        };
        push_cell(&mut cells, &scrape, widths.scrape, true);
        push_cell(
            &mut cells,
            &format!("{}/{}", self.success_counter, self.failed_counter),
            widths.counters,
            true,
        );
        cells
    }

    /// One rendered line, including the focus marker for the selected row.
    pub fn render_line(&self, widths: &TrackerColumnWidths, focused: bool) -> String {
        let marker = if focused { "* " } else { "  " };
        join_cells(marker, &self.cells(widths))
    }
}

/// Configurable widths for the tracker-list columns. A zero width hides the
/// column; the URL column is always kept because it identifies the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackerColumnWidths {
    pub group: usize,
    pub url: usize,
    pub tracker_type: usize,
    pub state: usize,
    pub event: usize,
    pub next: usize,
    pub scrape: usize,
    pub counters: usize,
}

impl Default for TrackerColumnWidths {
    fn default() -> Self {
        Self::full()
    }
}

impl TrackerColumnWidths {
    pub const fn full() -> Self {
        Self {
            group: 5,
            url: 40,
            tracker_type: 4,
            state: 4,
            event: 9,
            next: 7,
            scrape: 11,
            counters: 9,
        }
    }

    /// Pick a column set that fits `width` terminal columns. The URL always
    /// receives the remaining space so hosts stay readable on narrow screens.
    pub fn for_width(width: usize) -> Self {
        let (group, tracker_type, state, event, next, scrape, counters) = if width >= 100 {
            (5, 4, 4, 9, 7, 11, 9)
        } else if width >= 78 {
            (5, 4, 4, 9, 7, 0, 0)
        } else if width >= 56 {
            (5, 4, 4, 9, 6, 0, 0)
        } else if width >= 38 {
            (5, 4, 4, 0, 6, 0, 0)
        } else if width >= 24 {
            (0, 4, 4, 0, 0, 0, 0)
        } else {
            (0, 0, 0, 0, 0, 0, 0)
        };

        let fixed = group + tracker_type + state + event + next + scrape + counters;
        let visible = [group, tracker_type, state, event, next, scrape, counters]
            .iter()
            .filter(|w| **w > 0)
            .count();
        // Two marker columns, plus one separator between every visible cell.
        let used = 2 + fixed + visible;
        let url = width.saturating_sub(used).max(4);

        Self {
            group,
            url,
            tracker_type,
            state,
            event,
            next,
            scrape,
            counters,
        }
    }

    pub fn header_cells(&self) -> Vec<String> {
        let mut cells = Vec::with_capacity(8);
        push_cell(&mut cells, "Grp", self.group, false);
        push_cell(&mut cells, "URL", self.url, false);
        push_cell(&mut cells, "Type", self.tracker_type, false);
        push_cell(&mut cells, "St", self.state, false);
        push_cell(&mut cells, "Event", self.event, false);
        push_cell(&mut cells, "Next", self.next, true);
        push_cell(&mut cells, "S/I/D", self.scrape, true);
        push_cell(&mut cells, "OK/Err", self.counters, true);
        cells
    }

    pub fn header_line(&self) -> String {
        join_cells("  ", &self.header_cells())
    }
}

/// Actions available on the selected tracker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackerAction {
    ToggleEnabled,
    ForceAnnounce,
    ForceScrape,
}

impl TrackerAction {
    /// Key bindings while the tracker view is focused. `a`/`t` force an
    /// announce, `s` forces a scrape and space/`*` toggles enablement.
    pub fn for_key(key: i32) -> Option<Self> {
        match key {
            k if k == b' ' as i32 || k == b'*' as i32 => Some(TrackerAction::ToggleEnabled),
            k if k == b'a' as i32 || k == b't' as i32 => Some(TrackerAction::ForceAnnounce),
            k if k == b's' as i32 => Some(TrackerAction::ForceScrape),
            _ => None,
        }
    }
}

/// Result of a tracker action. Tracker lists can change between the snapshot
/// and the action, so stale selections are reported instead of panicking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackerActionOutcome {
    Applied,
    NoSelection,
    NoDownload,
    TrackerGone,
    Unavailable,
}

impl TrackerActionOutcome {
    pub fn is_applied(self) -> bool {
        self == TrackerActionOutcome::Applied
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ControllerSummary {
    key: u32,
    requesting: bool,
    promiscuous: bool,
    failure_mode: bool,
}

/// Standalone tracker-list window.
pub struct TrackerListWindow {
    pub flags: u32,
    pub download: Option<Arc<RwLock<Download>>>,
    pub download_name: String,
    pub rows: Vec<TrackerRow>,
    pub selected: usize,
    pub scroll_offset: usize,
    pub page_size: usize,
    /// `None` means use [`TrackerColumnWidths::for_width`] at draw time.
    pub column_widths: Option<TrackerColumnWidths>,
    /// Re-snapshot the live download on every frame, keeping announce/scrape
    /// completions and backoff timers current without external signals.
    pub auto_refresh: bool,
    /// Throttling interval for auto-refreshing tracker status (default 1.0s).
    pub refresh_interval: std::time::Duration,
    last_refresh: Option<std::time::Instant>,
    pub controller_key: u32,
    pub controller_requesting: bool,
    pub controller_promiscuous: bool,
    pub controller_failure_mode: bool,
    dirty: bool,
}

impl Default for TrackerListWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl TrackerListWindow {
    pub fn new() -> Self {
        Self {
            flags: 0,
            download: None,
            download_name: String::new(),
            rows: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            page_size: 10,
            column_widths: None,
            auto_refresh: true,
            refresh_interval: std::time::Duration::from_secs(1),
            last_refresh: None,
            controller_key: 0,
            controller_requesting: false,
            controller_promiscuous: false,
            controller_failure_mode: false,
            dirty: true,
        }
    }

    pub fn set_download(&mut self, download: Option<Arc<RwLock<Download>>>) {
        let changed = match (&self.download, &download) {
            (Some(old), Some(new)) => !Arc::ptr_eq(old, new),
            (None, None) => false,
            _ => true,
        };
        if changed {
            self.rows.clear();
            self.selected = 0;
            self.scroll_offset = 0;
        }
        self.download = download;
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

    /// Install a detached snapshot. Useful for tests and callers that already
    /// have their own tracker view model.
    pub fn set_trackers(&mut self, trackers: Vec<TrackerRow>) {
        self.rows = trackers;
        self.normalize_selection();
        self.scroll_offset = 0;
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    pub fn set_download_name(&mut self, name: impl Into<String>) {
        self.download_name = name.into();
        self.dirty = true;
    }

    pub fn set_column_widths(&mut self, widths: Option<TrackerColumnWidths>) {
        self.column_widths = widths;
        self.dirty = true;
    }

    pub fn set_auto_refresh(&mut self, auto_refresh: bool) {
        self.auto_refresh = auto_refresh;
        self.dirty = true;
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected_tracker(&self) -> Option<&TrackerRow> {
        self.rows.get(self.selected)
    }

    pub fn tracker_at(&self, index: usize) -> Option<&TrackerRow> {
        self.rows.get(index)
    }

    /// Re-snapshot the live download's `TrackerList`. Selection is preserved by
    /// group + URL because tracker indices may change when entries are added or
    /// removed.
    pub fn refresh(&mut self) {
        self.last_refresh = Some(std::time::Instant::now());
        let previous = self
            .selected_tracker()
            .map(|row| (row.group, row.url.clone()));

        match self.snapshot_live_trackers() {
            Some((name, rows, summary)) => {
                self.download_name = name;
                self.rows = rows;
                self.controller_key = summary.key;
                self.controller_requesting = summary.requesting;
                self.controller_promiscuous = summary.promiscuous;
                self.controller_failure_mode = summary.failure_mode;
                self.restore_selection(previous);
            }
            None => {
                self.download_name.clear();
                self.rows.clear();
                self.selected = 0;
                self.scroll_offset = 0;
                self.controller_key = 0;
                self.controller_requesting = false;
                self.controller_promiscuous = false;
                self.controller_failure_mode = false;
            }
        }
        self.normalize_selection();
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    pub fn select_index(&mut self, index: usize) -> bool {
        if index >= self.rows.len() {
            return false;
        }
        self.selected = index;
        self.ensure_visible(self.page_size);
        self.dirty = true;
        true
    }

    pub fn select_first(&mut self) {
        if !self.rows.is_empty() {
            self.select_index(0);
        }
    }

    pub fn select_last(&mut self) {
        if !self.rows.is_empty() {
            self.select_index(self.rows.len() - 1);
        }
    }

    /// Move down one row, wrapping at the end (legacy tracker-list behavior).
    pub fn select_next(&mut self) {
        self.move_selection(1);
    }

    /// Move up one row, wrapping at the start (legacy tracker-list behavior).
    pub fn select_prev(&mut self) {
        self.move_selection(-1);
    }

    pub fn page_down(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        let page = self.page_size.max(1);
        self.selected = (self.selected + page).min(self.rows.len() - 1);
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    pub fn page_up(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        let page = self.page_size.max(1);
        self.selected = self.selected.saturating_sub(page);
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    /// Keep `selected` inside the visible window of `visible_rows` rows.
    pub fn ensure_visible(&mut self, visible_rows: usize) {
        if visible_rows == 0 || self.rows.is_empty() {
            return;
        }
        self.normalize_selection();
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + visible_rows {
            self.scroll_offset = self.selected + 1 - visible_rows;
        }
        let max_offset = self.rows.len().saturating_sub(visible_rows);
        self.scroll_offset = self.scroll_offset.min(max_offset);
    }

    /// Handle an action key, if it maps to one. Returns `None` for unrelated
    /// keys so the caller can continue normal key processing.
    pub fn handle_action_key(&mut self, key: i32) -> Option<TrackerActionOutcome> {
        let action = TrackerAction::for_key(key)?;
        Some(self.apply_selected_action(action))
    }

    pub fn apply_selected_action(&mut self, action: TrackerAction) -> TrackerActionOutcome {
        let row = match self.selected_tracker() {
            Some(row) => row.clone(),
            None => return TrackerActionOutcome::NoSelection,
        };
        let download = match &self.download {
            Some(download) => Arc::clone(download),
            None => return TrackerActionOutcome::NoDownload,
        };

        let outcome = match action {
            TrackerAction::ToggleEnabled => toggle_enabled_via_command(&download, &row),
            TrackerAction::ForceAnnounce => force_announce(&download, &row),
            TrackerAction::ForceScrape => force_scrape(&download, &row),
        };

        if outcome.is_applied() {
            self.refresh();
        }
        outcome
    }

    fn restore_selection(&mut self, previous: Option<(u32, String)>) {
        if let Some((group, url)) = previous {
            if let Some(pos) = self
                .rows
                .iter()
                .position(|row| row.group == group && row.url == url)
            {
                self.selected = pos;
            }
        }
        self.normalize_selection();
    }

    fn normalize_selection(&mut self) {
        if self.rows.is_empty() {
            self.selected = 0;
            self.scroll_offset = 0;
        } else if self.selected >= self.rows.len() {
            self.selected = self.rows.len() - 1;
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let len = self.rows.len() as isize;
        let current = self.selected.min(self.rows.len() - 1) as isize;
        self.selected = (current + delta).rem_euclid(len) as usize;
        self.ensure_visible(self.page_size);
        self.dirty = true;
    }

    fn effective_widths(&self, width: u32) -> TrackerColumnWidths {
        self.column_widths
            .unwrap_or_else(|| TrackerColumnWidths::for_width(width as usize))
    }

    /// Snapshot the live tracker list without naming any `lt-tracker` type.
    fn snapshot_live_trackers(&self) -> Option<(String, Vec<TrackerRow>, ControllerSummary)> {
        let download = self.download.as_ref()?;
        let download = download.read().ok()?;
        let name = download.name().to_string();

        let engine = download.download();
        let controller = engine.tracker_controller();
        let list = &controller.tracker_list;
        let now = controller.cached_time();

        let summary = ControllerSummary {
            key: list.key,
            requesting: controller.is_requesting(),
            promiscuous: controller.is_promiscuous_mode(),
            failure_mode: controller.is_failure_mode(),
        };

        let mut rows = Vec::with_capacity(list.size());
        let mut last_group: Option<u32> = None;
        let mut group_index = 0usize;

        for index in 0..list.size() {
            let tracker = match list.at(index) {
                Some(tracker) => tracker,
                None => continue,
            };
            let state = tracker.state();
            let group = tracker.group();
            if last_group == Some(group) {
                group_index += 1;
            } else {
                last_group = Some(group);
                group_index = 0;
            }

            let next_announce_in =
                if state.counters.success_counter == 0 && state.counters.failed_counter == 0 {
                    None
                } else {
                    Some(state.activity_time_next().saturating_sub(now))
                };

            rows.push(TrackerRow {
                index,
                group_index,
                group,
                url: tracker.url().to_string(),
                tracker_type: TrackerTypeView::from_url(tracker.url()),
                enabled: state.is_enabled(),
                usable: tracker.is_usable(),
                busy: tracker.is_requesting(),
                in_use: tracker.is_in_use(),
                extra_tracker: state.is_extra_tracker(),
                scrapable: tracker.is_scrapable(),
                latest_event: state.latest_event as i64,
                normal_interval: state.normal_interval,
                min_interval: state.min_interval,
                success_counter: state.counters.success_counter,
                failed_counter: state.counters.failed_counter,
                scrape_counter: state.counters.scrape_counter,
                next_announce_in,
                latest_new_peers: state.latest_new_peers,
                latest_sum_peers: state.latest_sum_peers,
                scrape_complete: state.scrape_complete,
                scrape_incomplete: state.scrape_incomplete,
                scrape_downloaded: state.scrape_downloaded,
            });
        }

        Some((name, rows, summary))
    }
}

impl Window for TrackerListWindow {
    fn flags(&self) -> u32 {
        self.flags
    }

    fn set_flags(&mut self, flags: u32) {
        self.flags = flags;
    }

    fn min_width(&self) -> u32 {
        12
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
        self.page_size = canvas.height().saturating_sub(LIST_HEADER_ROWS).max(1) as usize;
        self.normalize_selection();

        if canvas.height() == 0 {
            self.dirty = false;
            return;
        }

        if self.download.is_none() && self.rows.is_empty() {
            canvas.print_at(0, 0, "[No download selected]");
            self.dirty = false;
            return;
        }

        let name = if self.download_name.is_empty() {
            "?"
        } else {
            self.download_name.as_str()
        };
        let title = format!(
            "[Trackers: {}] {} [key: {:08x}] [{}{}{}]",
            name,
            self.rows.len(),
            self.controller_key,
            if self.controller_requesting {
                "req"
            } else {
                "   "
            },
            if self.controller_promiscuous {
                " prom"
            } else {
                "     "
            },
            if self.controller_failure_mode {
                " fail"
            } else {
                "     "
            },
        );
        canvas.print_at(0, 0, &title);

        if self.rows.is_empty() {
            if canvas.height() > 1 {
                canvas.print_at(0, 1, "[No trackers configured]");
            }
            self.dirty = false;
            return;
        }

        if canvas.height() < 2 {
            self.dirty = false;
            return;
        }

        let widths = self.effective_widths(canvas.width());
        canvas.print_at_styled(
            0,
            1,
            &widths.header_line(),
            Style::default()
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::UNDERLINED),
        );

        self.ensure_visible(self.page_size);
        let end = (self.scroll_offset + self.page_size).min(self.rows.len());
        for (row_offset, tracker) in self.rows[self.scroll_offset..end].iter().enumerate() {
            let y = LIST_HEADER_ROWS + row_offset as u32;
            if y >= canvas.height() {
                break;
            }
            let focused = self.scroll_offset + row_offset == self.selected;
            let line = tracker.render_line(&widths, focused);
            let style = if focused {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else if !tracker.enabled {
                Style::default().fg(Color::DarkGray)
            } else if !tracker.usable {
                Style::default().fg(Color::Red)
            } else if tracker.busy {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            canvas.print_at_styled(0, y, &line, style);
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

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

fn toggle_enabled_via_command(
    download: &Arc<RwLock<Download>>,
    row: &TrackerRow,
) -> TrackerActionOutcome {
    // Re-resolve the index by URL: tracker indices are per-download and can
    // shift when entries are added or removed between snapshot and action.
    let (hash, live_index, live_enabled) = match download.read() {
        Ok(download) => {
            let hash = *download.info_hash();
            let engine = download.download();
            let list = &engine.tracker_controller().tracker_list;
            let mut found = None;
            for index in 0..list.size() {
                if let Some(tracker) = list.at(index) {
                    if tracker.url() == row.url && tracker.group() == row.group {
                        found = Some((index, tracker.state().is_enabled()));
                        break;
                    }
                }
            }
            match found {
                Some((index, enabled)) => (hash, index, enabled),
                None => return TrackerActionOutcome::TrackerGone,
            }
        }
        Err(_) => return TrackerActionOutcome::Unavailable,
    };

    // A temporary registry keeps this window independent from the global
    // command map while reusing the canonical `t.enable`/`t.disable` commands.
    let mut downloads = DownloadList::new();
    downloads.insert(Arc::clone(download));
    let downloads = Arc::new(RwLock::new(downloads));

    let mut map = CommandMap::new();
    let dht_config = Arc::new(RwLock::new(DhtConfig::new()));
    if register_tracker_commands(&mut map, dht_config, downloads).is_err() {
        return TrackerActionOutcome::Unavailable;
    }

    let command = if live_enabled {
        "t.disable"
    } else {
        "t.enable"
    };
    let target = Target::Tracker(hash, live_index as u32);
    match map.call_command(command, &Object::create_empty(), &target, true) {
        Ok(_) => TrackerActionOutcome::Applied,
        Err(_) => TrackerActionOutcome::TrackerGone,
    }
}

fn force_announce(download: &Arc<RwLock<Download>>, row: &TrackerRow) -> TrackerActionOutcome {
    let mut guard = match download.write() {
        Ok(guard) => guard,
        Err(_) => return TrackerActionOutcome::Unavailable,
    };
    let mut engine = guard.download_mut();
    let controller = engine.tracker_controller_mut();

    let mut live = None;
    for index in 0..controller.tracker_list.size() {
        if let Some(tracker) = controller.tracker_list.at(index) {
            if tracker.url() == row.url && tracker.group() == row.group {
                // `TrackerEvent` is not re-exported by rt-core/rt-tui. A fresh
                // default state carries `TrackerEvent::None`, which is exactly
                // the event a forced update announce uses.
                live = Some((
                    index,
                    tracker.is_usable(),
                    default_value(tracker.state()).latest_event,
                ));
                break;
            }
        }
    }

    match live {
        Some((index, true, none_event)) => {
            controller.tracker_list.send_event(index, none_event);
            TrackerActionOutcome::Applied
        }
        Some((_, false, _)) => TrackerActionOutcome::Unavailable,
        None => TrackerActionOutcome::TrackerGone,
    }
}

fn force_scrape(download: &Arc<RwLock<Download>>, row: &TrackerRow) -> TrackerActionOutcome {
    let mut guard = match download.write() {
        Ok(guard) => guard,
        Err(_) => return TrackerActionOutcome::Unavailable,
    };
    let mut engine = guard.download_mut();
    let controller = engine.tracker_controller_mut();

    let mut live = None;
    for index in 0..controller.tracker_list.size() {
        if let Some(tracker) = controller.tracker_list.at(index) {
            if tracker.url() == row.url && tracker.group() == row.group {
                live = Some((index, tracker.is_usable() && tracker.is_scrapable()));
                break;
            }
        }
    }

    match live {
        Some((index, true)) => {
            if controller.tracker_list.force_scrape(index) {
                TrackerActionOutcome::Applied
            } else {
                TrackerActionOutcome::Unavailable
            }
        }
        Some((_, false)) => TrackerActionOutcome::Unavailable,
        None => TrackerActionOutcome::TrackerGone,
    }
}

/// Build a default value for an inferred type. Used to obtain
/// `TrackerEvent::None` without naming the `lt-tracker` enum in this crate.
fn default_value<T: Default>(_: &T) -> T {
    T::default()
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

fn format_next(seconds: u64) -> String {
    if seconds == 0 {
        "now".to_string()
    } else if seconds < 60 {
        format!("{}s", seconds)
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else {
        format!("{}h", seconds / 3600)
    }
}

fn push_cell(cells: &mut Vec<String>, text: &str, width: usize, right_align: bool) {
    if width == 0 {
        return;
    }
    cells.push(fit(text, width, right_align));
}

fn push_cell_url(cells: &mut Vec<String>, url: &str, width: usize) {
    if width == 0 {
        return;
    }
    cells.push(fit_url(url, width));
}

fn fit(text: &str, width: usize, right_align: bool) -> String {
    let count = text.chars().count();
    let truncated: String = if count > width {
        let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
        out.push('…');
        out
    } else {
        text.to_string()
    };
    if right_align {
        format!("{:>width$}", truncated, width = width)
    } else {
        format!("{:<width$}", truncated, width = width)
    }
}

/// Truncate a URL while keeping the scheme and host visible. When the host
/// alone does not fit, fall back to plain character-safe truncation.
fn fit_url(url: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let count = url.chars().count();
    if count <= width {
        return format!("{:<width$}", url, width = width);
    }

    let host_prefix = url
        .split_once("://")
        .map(|(scheme, rest)| {
            let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            format!("{}://{}", scheme, &rest[..host_end])
        })
        .unwrap_or_else(|| url.to_string());

    let prefix_len = host_prefix.chars().count();
    let text = if prefix_len < width {
        let mut out = host_prefix;
        out.push('…');
        out
    } else {
        let mut out: String = url.chars().take(width.saturating_sub(1)).collect();
        out.push('…');
        out
    };

    let len = text.chars().count();
    if len < width {
        format!("{}{}", text, " ".repeat(width - len))
    } else {
        text
    }
}

/// Compose cells through the shared `TextElement` primitives.
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
pub fn tracker_list_window() -> TrackerListWindow {
    let mut window = TrackerListWindow::new();
    window.flags = FLAG_ACTIVE;
    window
}
